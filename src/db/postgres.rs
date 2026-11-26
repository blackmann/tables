//! PostgreSQL through the synchronous `postgres` client.
//!
//! Rows are read with the simple (text) protocol, after preparing the statement
//! to learn its column types: every Postgres type then has a faithful text form
//! and we decide how to show it from the type, without a Rust mapping per type.
use super::config::{ConnectionConfig, SslMode};
use super::*;
use anyhow::{Context as _, anyhow};
use ::postgres::{CancelToken, Client, SimpleQueryMessage, types::Type};
use postgres_native_tls::MakeTlsConnector;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct Postgres {
    client: Client,
    /// Shared with the canceller, so it follows the connection across reconnects.
    cancel: Arc<Mutex<CancelToken>>,
    tls: MakeTlsConnector,
    /// What we connected with, to connect again when the server drops us.
    config: ::postgres::Config,
    /// When we last knew the link was good.
    checked: Instant,
}

/// A connection idle longer than this is pinged before use: the client only
/// learns the server dropped it when a query fails.
const IDLE: Duration = Duration::from_secs(20);

impl Postgres {
    pub fn connect(c: &ConnectionConfig, password: Option<String>) -> Result<Postgres> {
        let mut config = ::postgres::Config::new();
        config
            .host(&c.host)
            .port(c.port)
            .user(&c.user)
            .application_name("Tables")
            .connect_timeout(Duration::from_secs(10))
            // Notice a dead link (sleep, network change) in under a minute rather than hours.
            .keepalives_idle(Duration::from_secs(30));
        if !c.database.is_empty() {
            config.dbname(&c.database);
        }
        if let Some(p) = password.filter(|p| !p.is_empty()) {
            config.password(p);
        }
        config.ssl_mode(match c.ssl {
            SslMode::Disable => ::postgres::config::SslMode::Disable,
            SslMode::Prefer => ::postgres::config::SslMode::Prefer,
            SslMode::Require | SslMode::RequireNoVerify => ::postgres::config::SslMode::Require,
        });
        let connector = native_tls::TlsConnector::builder()
            .danger_accept_invalid_certs(c.ssl == SslMode::RequireNoVerify)
            .danger_accept_invalid_hostnames(c.ssl == SslMode::RequireNoVerify)
            .build()?;
        let tls = MakeTlsConnector::new(connector);
        let client = config.connect(tls.clone()).map_err(|e| anyhow!(describe(&e)))?;
        let cancel = Arc::new(Mutex::new(client.cancel_token()));
        Ok(Postgres { client, cancel, tls, config, checked: Instant::now() })
    }

    /// A way to stop whatever this connection is running, usable from another thread.
    pub fn canceller(&self) -> Box<dyn Fn() + Send + Sync> {
        let token = self.cancel.clone();
        let tls = self.tls.clone();
        Box::new(move || {
            let token = token.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let _ = token.cancel_query(tls.clone());
        })
    }

    fn reconnect_now(&mut self) -> Result<()> {
        let client = self.config.connect(self.tls.clone()).map_err(|e| anyhow!("Lost the connection and couldn't reconnect: {}", describe(&e)))?;
        *self.cancel.lock().unwrap_or_else(|e| e.into_inner()) = client.cancel_token();
        self.client = client;
        self.checked = Instant::now();
        Ok(())
    }

    fn alive(&mut self) -> bool {
        let ok = !self.client.is_closed() && self.client.is_valid(Duration::from_secs(3)).is_ok();
        if ok {
            self.checked = Instant::now();
        }
        ok
    }

    /// Connect again if the server has dropped us. Called before each piece of
    /// work, never midway through a script or transaction.
    fn ensure(&mut self) -> Result<()> {
        let dead = self.client.is_closed() || (self.checked.elapsed() > IDLE && !self.alive());
        if dead {
            return self.reconnect_now();
        }
        self.checked = Instant::now();
        Ok(())
    }

    /// Run a read the app wrote itself, once more on a fresh connection if the
    /// server turns out to have dropped us. Never for the user's SQL.
    fn retrying<T>(&mut self, f: impl Fn(&mut Self) -> Result<T>) -> Result<T> {
        match f(self) {
            Err(_) if self.client.is_closed() => {
                self.reconnect_now()?;
                f(self)
            }
            r => r,
        }
    }

    fn text_rows(&mut self, sql: &str) -> Result<Vec<Vec<Option<String>>>> {
        self.retrying(|d| d.text_rows_once(sql))
    }

    fn text_rows_once(&mut self, sql: &str) -> Result<Vec<Vec<Option<String>>>> {
        self.ensure()?;
        let mut out = Vec::new();
        for m in self.client.simple_query(sql).map_err(|e| anyhow!(describe(&e)))? {
            if let SimpleQueryMessage::Row(r) = m {
                out.push((0..r.len()).map(|i| r.get(i).map(String::from)).collect());
            }
        }
        Ok(out)
    }

    fn read(&mut self, sql: &str) -> Result<Page> {
        self.ensure()?;
        let start = Instant::now();
        let stmt = self.client.prepare(sql).map_err(|e| anyhow!(describe(&e)))?;
        let types: Vec<Type> = stmt.columns().iter().map(|c| c.type_().clone()).collect();
        let columns = stmt.columns().iter().map(|c| ResultColumn { name: c.name().to_string(), type_name: type_label(c.type_()) }).collect();
        let rows = self
            .text_rows_once(sql)?
            .into_iter()
            .map(|r| r.into_iter().zip(&types).map(|(v, t)| convert(v, t)).collect())
            .collect();
        Ok(Page { columns, rows, elapsed: start.elapsed() })
    }
}

/// Postgres' own message, with its detail and hint, rather than the client's wrapper.
fn describe(e: &::postgres::Error) -> String {
    match e.as_db_error() {
        Some(db) => {
            let mut s = db.message().to_string();
            if let Some(d) = db.detail() {
                s += &format!("\n{d}");
            }
            if let Some(h) = db.hint() {
                s += &format!("\nHint: {h}");
            }
            s
        }
        None => {
            let s = e.to_string();
            let mut src: Option<&dyn std::error::Error> = std::error::Error::source(e);
            let mut tail = String::new();
            while let Some(x) = src {
                tail = x.to_string();
                src = x.source();
            }
            if tail.is_empty() || s.contains(&tail) { s } else { format!("{s}: {tail}") }
        }
    }
}

fn type_label(t: &Type) -> String {
    match t.name() {
        "int2" | "int4" | "int8" | "float4" | "float8" | "bool" | "text" | "uuid" | "jsonb" | "json" | "date" | "numeric" | "bytea" => t.name().into(),
        "timestamptz" => "timestamptz".into(),
        "varchar" => "varchar".into(),
        "bpchar" => "char".into(),
        n if n.starts_with('_') => format!("{}[]", &n[1..]),
        n => n.into(),
    }
}

fn convert(v: Option<String>, t: &Type) -> Value {
    let Some(s) = v else { return Value::Null };
    match t.name() {
        "bool" => Value::Bool(s == "t"),
        "int2" | "int4" | "int8" | "oid" => s.parse().map(Value::Int).unwrap_or(Value::Text(s)),
        "float4" | "float8" => s.parse().map(Value::Float).unwrap_or(Value::Text(s)),
        "numeric" | "money" => Value::Number(s),
        "json" | "jsonb" => serde_json::from_str(&s).map(Value::Json).unwrap_or(Value::Text(s)),
        "bytea" => s
            .strip_prefix("\\x")
            .and_then(|h| (0..h.len()).step_by(2).map(|i| u8::from_str_radix(h.get(i..i + 2)?, 16).ok()).collect::<Option<Vec<u8>>>())
            .map(Value::Bytes)
            .unwrap_or(Value::Text(s)),
        _ => Value::Text(s),
    }
}

fn regclass(schema: &str, table: &str) -> String {
    format!("{}::regclass", quote_text(&qualified(Engine::Postgres, schema, table)))
}

impl Driver for Postgres {
    fn engine(&self) -> Engine {
        Engine::Postgres
    }
    fn version(&mut self) -> Result<String> {
        let v = self.text_rows("SHOW server_version")?.into_iter().next().and_then(|r| r.into_iter().next().flatten()).unwrap_or_default();
        Ok(format!("PostgreSQL {}", v.split_whitespace().next().unwrap_or("")))
    }
    fn schemas(&mut self) -> Result<Vec<String>> {
        Ok(self
            .text_rows("SELECT nspname FROM pg_namespace WHERE nspname NOT LIKE 'pg\\_%' AND nspname <> 'information_schema' ORDER BY nspname = 'public' DESC, nspname")?
            .into_iter()
            .filter_map(|r| r.into_iter().next().flatten())
            .collect())
    }
    fn default_schema(&self) -> String {
        "public".into()
    }
    fn is_closed(&self) -> bool {
        self.client.is_closed()
    }
    fn ping(&mut self) -> bool {
        self.alive()
    }
    fn reconnect(&mut self) -> Result<()> {
        self.reconnect_now()
    }
    fn relations(&mut self, schema: &str) -> Result<Vec<Relation>> {
        let sql = format!(
            "SELECT c.relname, c.relkind, c.reltuples::bigint FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = {} AND c.relkind IN ('r','p','v','m','f') AND NOT c.relispartition ORDER BY c.relname",
            quote_text(schema)
        );
        let mut out: Vec<Relation> = self
            .text_rows(&sql)?
            .into_iter()
            .map(|r| {
                let kind = if matches!(r[1].as_deref(), Some("v" | "m")) { RelationKind::View } else { RelationKind::Table };
                let rows = r[2].as_deref().and_then(|s| s.parse::<i64>().ok()).filter(|n| *n >= 0);
                Relation { name: r[0].clone().unwrap_or_default(), kind, rows }
            })
            .collect();
        // Small tables that were never analyzed report -1; count them for real.
        for rel in out.iter_mut().filter(|r| r.kind == RelationKind::Table && r.rows.is_none_or(|n| n == 0)).take(40) {
            if let Ok(n) = self.count(schema, &rel.name.clone()) {
                rel.rows = Some(n);
            }
        }
        Ok(out)
    }
    fn functions(&mut self, schema: &str) -> Result<Vec<(String, String)>> {
        let sql = format!(
            "SELECT p.proname, '(' || pg_get_function_identity_arguments(p.oid) || ')' FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace \
             WHERE n.nspname = {} AND p.prokind IN ('f','p') ORDER BY 1",
            quote_text(schema)
        );
        Ok(self.text_rows(&sql)?.into_iter().map(|r| (r[0].clone().unwrap_or_default(), r[1].clone().unwrap_or_default())).collect())
    }
    fn detail(&mut self, schema: &str, table: &str) -> Result<TableDetail> {
        let rc = regclass(schema, table);
        let cols = self.text_rows(&format!(
            "SELECT a.attname, format_type(a.atttypid, a.atttypmod), (NOT a.attnotnull)::text, pg_get_expr(d.adbin, d.adrelid), col_description(a.attrelid, a.attnum), \
             (SELECT string_agg(e.enumlabel, E'\\x1f' ORDER BY e.enumsortorder) FROM pg_enum e WHERE e.enumtypid = a.atttypid) \
             FROM pg_attribute a LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
             WHERE a.attrelid = {rc} AND a.attnum > 0 AND NOT a.attisdropped ORDER BY a.attnum"
        ))?;
        let cons = self.text_rows(&format!(
            "SELECT con.contype, array_to_string(array(SELECT a.attname FROM unnest(con.conkey) WITH ORDINALITY k(n, o) JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = k.n ORDER BY k.o), ','), \
             cf.relname, array_to_string(array(SELECT a.attname FROM unnest(con.confkey) WITH ORDINALITY k(n, o) JOIN pg_attribute a ON a.attrelid = con.confrelid AND a.attnum = k.n ORDER BY k.o), ','), \
             con.conname, pg_get_constraintdef(con.oid, true) \
             FROM pg_constraint con LEFT JOIN pg_class cf ON cf.oid = con.confrelid WHERE con.conrelid = {rc} ORDER BY con.contype, con.conname"
        ))?;
        let mut columns: Vec<ColumnInfo> = cols
            .into_iter()
            .map(|r| ColumnInfo {
                name: r[0].clone().unwrap_or_default(),
                data_type: r[1].clone().unwrap_or_default(),
                nullable: r[2].as_deref() == Some("true"),
                default: r[3].clone(),
                comment: r[4].clone(),
                enum_values: r[5].as_deref().map(|s| s.split('\x1f').map(String::from).collect()).unwrap_or_default(),
                ..Default::default()
            })
            .collect();
        let mut constraint_ddl = Vec::new();
        for c in &cons {
            let kind = c[0].as_deref().unwrap_or("");
            let keys: Vec<&str> = c[1].as_deref().unwrap_or("").split(',').filter(|s| !s.is_empty()).collect();
            for col in columns.iter_mut() {
                match kind {
                    "p" if keys.contains(&col.name.as_str()) => col.primary_key = true,
                    "u" if keys.len() == 1 && keys[0] == col.name => col.unique = true,
                    "f" if keys.len() == 1 && keys[0] == col.name => {
                        col.foreign = Some((c[2].clone().unwrap_or_default(), c[3].clone().unwrap_or_default()));
                    }
                    _ => {}
                }
            }
            constraint_ddl.push(format!("  CONSTRAINT {} {}", Engine::Postgres.quote(c[4].as_deref().unwrap_or("")), c[5].as_deref().unwrap_or("")));
        }
        let idx = self.text_rows(&format!(
            "SELECT i.relname, ix.indisunique::text, ix.indisprimary::text, am.amname, pg_size_pretty(pg_relation_size(i.oid)), s.idx_scan, \
             array_to_string(array(SELECT pg_get_indexdef(ix.indexrelid, k + 1, true) FROM generate_subscripts(ix.indkey, 1) k ORDER BY k), ','), \
             pg_get_indexdef(ix.indexrelid), EXISTS (SELECT 1 FROM pg_constraint c WHERE c.conindid = ix.indexrelid)::text \
             FROM pg_index ix JOIN pg_class i ON i.oid = ix.indexrelid JOIN pg_am am ON am.oid = i.relam \
             LEFT JOIN pg_stat_all_indexes s ON s.indexrelid = ix.indexrelid \
             WHERE ix.indrelid = {rc} ORDER BY ix.indisprimary DESC, i.relname"
        ))?;
        let mut index_ddl = Vec::new();
        let indexes = idx
            .into_iter()
            .map(|r| {
                if r[8].as_deref() != Some("true") {
                    index_ddl.push(format!("{};", r[7].clone().unwrap_or_default()));
                }
                IndexInfo {
                    name: r[0].clone().unwrap_or_default(),
                    unique: r[1].as_deref() == Some("true"),
                    primary: r[2].as_deref() == Some("true"),
                    method: r[3].clone().unwrap_or_default(),
                    size: r[4].clone(),
                    scans: r[5].as_deref().and_then(|s| s.parse().ok()),
                    columns: r[6].as_deref().unwrap_or("").split(',').map(String::from).collect(),
                }
            })
            .collect();
        let referenced_by = self
            .text_rows(&format!(
                "SELECT c.relname, a.attname FROM pg_constraint con JOIN pg_class c ON c.oid = con.conrelid \
                 JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = con.conkey[1] \
                 WHERE con.confrelid = {rc} AND con.contype = 'f' ORDER BY 1, 2"
            ))?
            .into_iter()
            .map(|r| (r[0].clone().unwrap_or_default(), r[1].clone().unwrap_or_default()))
            .collect();
        let info = self.text_rows(&format!("SELECT pg_size_pretty(pg_total_relation_size({rc})), c.relkind, pg_get_viewdef({rc}, true) FROM pg_class c WHERE c.oid = {rc}"))?;
        let size = info.first().and_then(|r| r[0].clone());
        let is_view = info.first().is_some_and(|r| matches!(r[1].as_deref(), Some("v" | "m")));
        let name = qualified(Engine::Postgres, schema, table);
        let ddl = if is_view {
            format!("CREATE VIEW {name} AS\n{}", info.first().and_then(|r| r[2].clone()).unwrap_or_default())
        } else {
            let mut lines: Vec<String> = columns
                .iter()
                .map(|c| {
                    let mut l = format!("  {} {}", Engine::Postgres.quote(&c.name), c.data_type);
                    if !c.nullable {
                        l += " NOT NULL";
                    }
                    if let Some(d) = &c.default {
                        l += &format!(" DEFAULT {d}");
                    }
                    l
                })
                .collect();
            lines.extend(constraint_ddl);
            let mut ddl = format!("CREATE TABLE {name} (\n{}\n);", lines.join(",\n"));
            for i in index_ddl {
                ddl += "\n\n";
                ddl += &i;
            }
            ddl
        };
        Ok(TableDetail { columns, indexes, referenced_by, ddl, size })
    }
    fn all_columns(&mut self, schema: &str) -> Result<Vec<(String, String, String)>> {
        let sql = format!(
            "SELECT c.relname, a.attname, format_type(a.atttypid, a.atttypmod) FROM pg_attribute a JOIN pg_class c ON c.oid = a.attrelid \
             JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = {} AND c.relkind IN ('r','p','v','m','f') AND a.attnum > 0 AND NOT a.attisdropped \
             ORDER BY c.relname, a.attnum",
            quote_text(schema)
        );
        Ok(self.text_rows(&sql)?.into_iter().map(|r| (r[0].clone().unwrap_or_default(), r[1].clone().unwrap_or_default(), r[2].clone().unwrap_or_default())).collect())
    }
    fn query(&mut self, sql: &str) -> Result<Page> {
        // Only the app's own SELECTs come through here; scripts go through `execute`.
        self.retrying(|d| d.read(sql))
    }
    fn execute(&mut self, sql: &str) -> Result<Vec<(String, Outcome)>, (Vec<(String, Outcome)>, String, anyhow::Error)> {
        if let Err(e) = self.ensure() {
            return Err((vec![], split_statements(sql).into_iter().next().unwrap_or_default(), e));
        }
        let mut done = Vec::new();
        for stmt in split_statements(sql) {
            let start = Instant::now();
            // Statements that return rows are prepared first for their column types;
            // anything that can't be prepared (some utility commands) runs as is.
            let returns_rows = self.client.prepare(&stmt).map(|s| !s.columns().is_empty());
            let res = match returns_rows {
                Ok(true) => self.read(&stmt).map(Outcome::Rows),
                _ => self.client.simple_query(&stmt).map_err(|e| anyhow!(describe(&e))).map(|msgs| {
                    let n = msgs.iter().rev().find_map(|m| if let SimpleQueryMessage::CommandComplete(n) = m { Some(*n) } else { None });
                    let command = command_of(&stmt);
                    let rows = matches!(command.as_str(), "INSERT" | "UPDATE" | "DELETE" | "MERGE" | "COPY").then_some(n).flatten();
                    Outcome::Done { command, rows, elapsed: start.elapsed() }
                }),
            };
            match res {
                Ok(o) => done.push((stmt, o)),
                Err(e) => return Err((done, stmt, e)),
            }
        }
        Ok(done)
    }
    fn apply(&mut self, statements: &[String]) -> Result<()> {
        self.ensure()?;
        let mut tx = self.client.transaction().map_err(|e| anyhow!(describe(&e)))?;
        for s in statements {
            tx.batch_execute(s).map_err(|e| anyhow!("{}\n\n{s}", describe(&e)))?;
        }
        tx.commit().map_err(|e| anyhow!(describe(&e)))?;
        Ok(())
    }
    fn explain(&mut self, sql: &str) -> Result<Plan> {
        let body = sql.trim().trim_end_matches(';');
        let analyze = reads_only(body);
        let q = if analyze { format!("EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {body}") } else { format!("EXPLAIN (FORMAT JSON) {body}") };
        let text = self.text_rows_once(&q)?.into_iter().next().and_then(|r| r.into_iter().next().flatten()).context("No plan returned")?;
        let json: serde_json::Value = serde_json::from_str(&text)?;
        let top = &json[0];
        let mut nodes = Vec::new();
        walk(&top["Plan"], 0, &mut nodes);
        let mut summary = String::new();
        if let Some(p) = top["Planning Time"].as_f64() {
            summary += &format!("Planning {p:.1} ms");
        }
        if let Some(e) = top["Execution Time"].as_f64() {
            summary += &format!(" · Execution {e:.1} ms");
        }
        if !analyze {
            summary += "Estimated plan — writes aren't run under EXPLAIN ANALYZE";
        }
        let advice = advise(&nodes, |s| Engine::Postgres.quote(s));
        Ok(Plan { nodes, summary, advice })
    }
}

impl Postgres {
    fn count(&mut self, schema: &str, table: &str) -> Result<i64> {
        let sql = format!("SELECT count(*) FROM {} {}", qualified(Engine::Postgres, schema, table), "");
        let n = self.text_rows(&sql)?.into_iter().next().and_then(|r| r.into_iter().next().flatten()).unwrap_or_default();
        n.parse().context("count")
    }
    pub fn into_parts(self) -> (Box<dyn Driver>, Box<dyn Fn() + Send + Sync>) {
        let c = self.canceller();
        (Box::new(self), c)
    }
}

fn walk(node: &serde_json::Value, depth: usize, out: &mut Vec<PlanNode>) {
    let loops = node["Actual Loops"].as_f64().unwrap_or(1.);
    let total = node["Actual Total Time"].as_f64().map(|t| t * loops);
    let children: Vec<&serde_json::Value> = node["Plans"].as_array().map(|a| a.iter().collect()).unwrap_or_default();
    let child_time: f64 = children.iter().filter_map(|c| c["Actual Total Time"].as_f64().map(|t| t * c["Actual Loops"].as_f64().unwrap_or(1.))).sum();
    let relation = node["Relation Name"].as_str().map(|r| match node["Alias"].as_str() {
        Some(a) if a != r => format!("{r} {a}"),
        _ => r.to_string(),
    });
    let join = |v: &serde_json::Value| v.as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", "));
    let mut detail = ["Index Cond", "Hash Cond", "Merge Cond", "Filter", "Join Filter", "Recheck Cond"]
        .iter()
        .find_map(|k| node[*k].as_str().map(|v| format!("{k}: {v}")))
        .or_else(|| join(&node["Sort Key"]).map(|v| format!("Sort Key: {v}")))
        .or_else(|| join(&node["Group Key"]).map(|v| format!("Group Key: {v}")))
        .unwrap_or_default();
    if let Some(i) = node["Index Name"].as_str() {
        detail = format!("using {i}  {detail}");
    }
    let mut op = node["Node Type"].as_str().unwrap_or("?").to_string();
    if let Some(s) = node["Join Type"].as_str().filter(|s| *s != "Inner") {
        op = format!("{op} ({s})");
    }
    out.push(PlanNode {
        depth,
        operation: op,
        relation,
        detail,
        rows: node["Actual Rows"].as_f64().map(|r| r * loops).or_else(|| node["Plan Rows"].as_f64()),
        self_ms: total.map(|t| (t - child_time).max(0.)),
        removed_by_filter: node["Rows Removed by Filter"].as_f64().map(|r| r * loops),
    });
    for c in children {
        walk(c, depth + 1, out);
    }
}
