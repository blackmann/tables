//! ClickHouse over its HTTP interface (port 8123, or 8443 with TLS).
//!
//! Every request asks for `JSONCompact`, which carries each column's type next
//! to the data, so values are shown by type like the other engines. A session
//! id keeps `SET` and temporary tables alive between statements, and each
//! query gets its own id so another thread can `KILL QUERY` it.
use super::config::{ConnectionConfig, SslMode};
use super::*;
use anyhow::{Context as _, anyhow};
use std::io::Read as _;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone)]
struct Endpoint {
    agent: ureq::Agent,
    url: String,
    user: String,
    password: String,
    database: String,
    /// The user may not change settings (`readonly = 1`, or constraints): ask
    /// for the format in the query instead, and take the server's defaults.
    locked: bool,
}

pub struct ClickHouse {
    endpoint: Endpoint,
    session: String,
    /// The query running now, shared with the canceller.
    running: Arc<Mutex<Option<String>>>,
    next_id: u64,
}

/// What came back from one statement.
enum Reply {
    Rows(Page),
    /// No result set; `written` is how many rows an INSERT wrote.
    Done { written: Option<u64> },
}

impl Endpoint {
    /// POST `sql` with the given settings; the body on success, ClickHouse's own message on failure.
    fn post(&self, sql: &str, params: &[(&str, &str)]) -> Result<(String, Option<String>)> {
        let mut req = self
            .agent
            .post(&self.url)
            .set("X-ClickHouse-User", &self.user);
        if !self.locked {
            req = req
                .query("default_format", "JSONCompact")
                // Buffer the whole result so an error mid-query still comes back as an error.
                .query("wait_end_of_query", "1")
                .query("output_format_json_quote_64bit_integers", "1")
                .query("output_format_json_quote_decimals", "1")
                .query("output_format_json_quote_denormals", "1");
        }
        if !self.password.is_empty() {
            req = req.set("X-ClickHouse-Key", &self.password);
        }
        if !self.database.is_empty() {
            req = req.query("database", &self.database);
        }
        for (k, v) in params {
            req = req.query(k, v);
        }
        match req.send_string(sql) {
            Ok(r) => {
                let summary = r.header("X-ClickHouse-Summary").map(String::from);
                let mut body = String::new();
                r.into_reader().read_to_string(&mut body)?;
                Ok((body, summary))
            }
            Err(ureq::Error::Status(_, r)) => {
                let mut body = String::new();
                let _ = r.into_reader().read_to_string(&mut body);
                Err(anyhow!(tidy_error(&body)))
            }
            Err(ureq::Error::Transport(t)) => Err(anyhow!(describe_transport(&t))),
        }
    }
}

/// "Code: 60. DB::Exception: Table x doesn't exist. (UNKNOWN_TABLE) (version …)" → the sentence.
fn tidy_error(body: &str) -> String {
    let s = body.trim();
    let s = s.split_once("DB::Exception: ").map(|(_, r)| r).unwrap_or(s);
    let s = s.split(" (version ").next().unwrap_or(s);
    s.trim().to_string()
}

fn describe_transport(t: &ureq::Transport) -> String {
    let mut s = t.to_string();
    let mut src: Option<&dyn std::error::Error> = std::error::Error::source(t);
    while let Some(x) = src {
        let m = x.to_string();
        if !s.contains(&m) {
            s = format!("{s}: {m}");
        }
        src = x.source();
    }
    s
}

impl ClickHouse {
    pub fn connect(c: &ConnectionConfig, password: Option<String>) -> Result<ClickHouse> {
        let secure = matches!(c.ssl, SslMode::Require | SslMode::RequireNoVerify);
        let connector = native_tls::TlsConnector::builder()
            .danger_accept_invalid_certs(c.ssl == SslMode::RequireNoVerify)
            .danger_accept_invalid_hostnames(c.ssl == SslMode::RequireNoVerify)
            .build()?;
        let agent = ureq::AgentBuilder::new().tls_connector(Arc::new(connector)).timeout_connect(Duration::from_secs(10)).user_agent("Tables").build();
        let host = if c.host.is_empty() { "localhost" } else { c.host.as_str() };
        let port = if c.port == 0 { if secure { 8443 } else { 8123 } } else { c.port };
        let endpoint = Endpoint {
            agent,
            url: format!("{}://{host}:{port}/", if secure { "https" } else { "http" }),
            user: if c.user.is_empty() { "default".into() } else { c.user.clone() },
            password: password.unwrap_or_default(),
            database: c.database.clone(),
            locked: false,
        };
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let mut ch = ClickHouse { endpoint, session: format!("tables-{nanos:x}"), running: Arc::new(Mutex::new(None)), next_id: 0 };
        // Fail now, with the server's reason, rather than on the first query.
        if let Err(e) = ch.post("SELECT 1", &[]) {
            if !e.to_string().contains("READONLY") {
                return Err(e);
            }
            ch.endpoint.locked = true;
            ch.post("SELECT 1", &[])?;
        }
        Ok(ch)
    }

    /// A way to stop whatever this connection is running, usable from another thread.
    pub fn canceller(&self) -> Box<dyn Fn() + Send + Sync> {
        let running = self.running.clone();
        let mut endpoint = self.endpoint.clone();
        endpoint.database.clear();
        Box::new(move || {
            let id = running.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if let Some(id) = id {
                let _ = endpoint.post(&format!("KILL QUERY WHERE query_id = {} ASYNC", text(&id)), &[]);
            }
        })
    }

    pub fn into_parts(self) -> (Box<dyn Driver>, Box<dyn Fn() + Send + Sync>) {
        let c = self.canceller();
        (Box::new(self), c)
    }

    fn post(&mut self, sql: &str, extra: &[(&str, &str)]) -> Result<(String, Option<String>)> {
        let extra = if self.endpoint.locked { &[] } else { extra };
        self.next_id += 1;
        let id = format!("{}-{}", self.session, self.next_id);
        *self.running.lock().unwrap_or_else(|e| e.into_inner()) = Some(id.clone());
        let mut params = vec![("session_id", self.session.as_str()), ("session_timeout", "3600"), ("query_id", id.as_str())];
        params.extend_from_slice(extra);
        let res = self.endpoint.post(sql, &params);
        *self.running.lock().unwrap_or_else(|e| e.into_inner()) = None;
        res
    }

    fn run(&mut self, sql: &str, extra: &[(&str, &str)]) -> Result<Reply> {
        let start = Instant::now();
        let owned;
        let sql = if self.endpoint.locked && wants_format(sql) {
            owned = format!("{sql}\nFORMAT JSONCompact");
            owned.as_str()
        } else {
            sql
        };
        let (body, summary) = self.post(sql, extra)?;
        if body.trim().is_empty() {
            let written = summary.and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()).and_then(|j| j["written_rows"].as_str().and_then(|n| n.parse().ok()));
            return Ok(Reply::Done { written });
        }
        let mut page = parse(&body);
        page.elapsed = start.elapsed();
        Ok(Reply::Rows(page))
    }

    fn read(&mut self, sql: &str) -> Result<Page> {
        match self.run(sql, &[])? {
            Reply::Rows(p) => Ok(p),
            Reply::Done { .. } => Ok(Page::default()),
        }
    }

    /// Rows as text, for the app's own catalog queries.
    fn text_rows(&mut self, sql: &str) -> Result<Vec<Vec<Option<String>>>> {
        Ok(self.read(sql)?.rows.into_iter().map(|r| r.into_iter().map(|v| if v.is_null() { None } else { Some(v.display()) }).collect()).collect())
    }

    fn first(&mut self, sql: &str) -> Result<Option<String>> {
        Ok(self.text_rows(sql)?.into_iter().next().and_then(|r| r.into_iter().next().flatten()))
    }
}

/// Whether a statement returns rows and doesn't name its own output format.
fn wants_format(sql: &str) -> bool {
    let code = strip_comments(sql).to_ascii_lowercase();
    matches!(command_of(sql).as_str(), "SELECT" | "WITH" | "SHOW" | "DESCRIBE" | "DESC" | "EXPLAIN" | "EXISTS")
        && !code.split_whitespace().collect::<Vec<_>>().windows(2).any(|w| w[0] == "format" && !w[1].starts_with('('))
}

/// A string literal the way ClickHouse reads it: backslashes escape, so double them.
fn text(s: &str) -> String {
    quote_text(&s.replace('\\', "\\\\"))
}

/// Read a `JSONCompact` body; anything else (a user's own `FORMAT CSV`) becomes lines of text.
fn parse(body: &str) -> Page {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(body) else {
        return Page {
            columns: vec![ResultColumn { name: "result".into(), type_name: "String".into() }],
            rows: body.lines().map(|l| vec![Value::Text(l.to_string())]).collect(),
            elapsed: Duration::ZERO,
        };
    };
    let columns: Vec<ResultColumn> = json["meta"]
        .as_array()
        .map(|m| m.iter().map(|c| ResultColumn { name: c["name"].as_str().unwrap_or("").into(), type_name: c["type"].as_str().unwrap_or("").into() }).collect())
        .unwrap_or_default();
    let rows = json["data"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|r| {
                    let cells = r.as_array().cloned().unwrap_or_default();
                    cells.into_iter().zip(&columns).map(|(v, c)| convert(v, &c.type_name)).collect()
                })
                .collect()
        })
        .unwrap_or_default();
    Page { columns, rows, elapsed: Duration::ZERO }
}

/// The type inside `Nullable(…)` and `LowCardinality(…)`.
pub fn base_type(t: &str) -> &str {
    let mut t = t.trim();
    loop {
        match ["Nullable(", "LowCardinality("].iter().find_map(|p| t.strip_prefix(p)) {
            Some(inner) => t = inner.strip_suffix(')').unwrap_or(inner).trim(),
            None => return t,
        }
    }
}

fn convert(v: serde_json::Value, type_name: &str) -> Value {
    use serde_json::Value as J;
    let t = base_type(type_name);
    match v {
        J::Null => Value::Null,
        J::Bool(b) => Value::Bool(b),
        J::Number(n) => match n.as_i64() {
            Some(i) => Value::Int(i),
            None => n.as_f64().map(Value::Float).unwrap_or(Value::Number(n.to_string())),
        },
        J::String(s) if t.starts_with("Decimal") => Value::Number(s),
        // 64-bit and wider integers come quoted; keep the ones past i64 exact as text.
        J::String(s) if t.starts_with("Int") || t.starts_with("UInt") => s.parse().map(Value::Int).unwrap_or(Value::Number(s)),
        J::String(s) if t.starts_with("Float") => s.parse().map(Value::Float).unwrap_or(Value::Text(s)),
        J::String(s) if t.starts_with("JSON") || t.starts_with("Object") => serde_json::from_str(&s).map(Value::Json).unwrap_or(Value::Text(s)),
        J::String(s) => Value::Text(s),
        other => Value::Json(other),
    }
}

/// `Enum8('a' = 1, 'b' = 2)` → ["a", "b"].
fn enum_values(t: &str) -> Vec<String> {
    let t = base_type(t);
    if !t.starts_with("Enum") {
        return vec![];
    }
    let mut out = Vec::new();
    let mut chars = t.chars();
    while let Some(c) = chars.next() {
        if c != '\'' {
            continue;
        }
        let mut s = String::new();
        while let Some(c) = chars.next() {
            match c {
                '\\' => s.extend(chars.next()),
                '\'' => break,
                c => s.push(c),
            }
        }
        out.push(s);
    }
    out
}

impl Driver for ClickHouse {
    fn engine(&self) -> Engine {
        Engine::ClickHouse
    }
    fn version(&mut self) -> Result<String> {
        Ok(format!("ClickHouse {}", self.first("SELECT version()")?.unwrap_or_default()))
    }
    fn schemas(&mut self) -> Result<Vec<String>> {
        Ok(self
            .text_rows("SELECT name FROM system.databases WHERE lower(name) <> 'information_schema' ORDER BY name = 'system', name = 'default' DESC, name")?
            .into_iter()
            .filter_map(|r| r.into_iter().next().flatten())
            .collect())
    }
    fn default_schema(&self) -> String {
        if self.endpoint.database.is_empty() { "default".into() } else { self.endpoint.database.clone() }
    }
    fn ping(&mut self) -> bool {
        self.endpoint.agent.get(&format!("{}ping", self.endpoint.url)).timeout(Duration::from_secs(3)).call().is_ok()
    }
    fn relations(&mut self, schema: &str) -> Result<Vec<Relation>> {
        let sql = format!("SELECT name, engine, total_rows FROM system.tables WHERE database = {} AND NOT is_temporary ORDER BY name", text(schema));
        Ok(self
            .text_rows(&sql)?
            .into_iter()
            .map(|r| {
                let kind = if r[1].as_deref().is_some_and(|e| e.ends_with("View")) { RelationKind::View } else { RelationKind::Table };
                Relation { name: r[0].clone().unwrap_or_default(), kind, rows: r[2].as_deref().and_then(|s| s.parse().ok()) }
            })
            .collect())
    }
    fn functions(&mut self, _schema: &str) -> Result<Vec<(String, String)>> {
        // Only functions people defined: the built-ins number in the thousands.
        Ok(self
            .text_rows("SELECT name FROM system.functions WHERE origin = 'SQLUserDefined' ORDER BY name")
            .unwrap_or_default()
            .into_iter()
            .map(|r| (r[0].clone().unwrap_or_default(), String::new()))
            .collect())
    }
    fn detail(&mut self, schema: &str, table: &str) -> Result<TableDetail> {
        let (s, t) = (text(schema), text(table));
        let cols = self.text_rows(&format!(
            "SELECT name, type, default_kind, default_expression, comment, is_in_primary_key FROM system.columns WHERE database = {s} AND table = {t} ORDER BY position"
        ))?;
        let columns: Vec<ColumnInfo> = cols
            .into_iter()
            .map(|r| {
                let data_type = r[1].clone().unwrap_or_default();
                let default = match (r[2].as_deref().unwrap_or(""), r[3].clone().unwrap_or_default()) {
                    ("", _) => None,
                    ("DEFAULT", e) => Some(e),
                    (kind, e) => Some(format!("{kind} {e}")),
                };
                ColumnInfo {
                    name: r[0].clone().unwrap_or_default(),
                    nullable: data_type.starts_with("Nullable(") || data_type.starts_with("LowCardinality(Nullable("),
                    enum_values: enum_values(&data_type),
                    data_type,
                    default,
                    comment: r[4].clone().filter(|c| !c.is_empty()),
                    primary_key: r[5].as_deref() == Some("1"),
                    ..Default::default()
                }
            })
            .collect();
        let info = self.text_rows(&format!("SELECT primary_key, sorting_key, formatReadableSize(total_bytes), engine FROM system.tables WHERE database = {s} AND name = {t}"))?;
        let info = info.into_iter().next().unwrap_or_default();
        let at = |i: usize| info.get(i).cloned().flatten().filter(|v| !v.is_empty());
        let mut indexes = Vec::new();
        if let Some(pk) = at(0).or_else(|| at(1)) {
            indexes.push(IndexInfo {
                name: "PRIMARY KEY".into(),
                columns: pk.split(", ").map(String::from).collect(),
                unique: false,
                primary: true,
                method: at(3).unwrap_or_else(|| "sparse".into()),
                size: None,
                scans: None,
            });
        }
        // Data-skipping indexes; older servers don't have the table.
        for r in self
            .text_rows(&format!("SELECT name, type, expr, formatReadableSize(data_compressed_bytes) FROM system.data_skipping_indices WHERE database = {s} AND table = {t} ORDER BY name"))
            .unwrap_or_default()
        {
            indexes.push(IndexInfo {
                name: r[0].clone().unwrap_or_default(),
                columns: vec![r[2].clone().unwrap_or_default()],
                unique: false,
                primary: false,
                method: r[1].clone().unwrap_or_default(),
                size: r[3].clone(),
                scans: None,
            });
        }
        let ddl = self.first(&format!("SHOW CREATE TABLE {}", qualified(Engine::ClickHouse, schema, table))).ok().flatten().unwrap_or_default();
        Ok(TableDetail { columns, indexes, referenced_by: vec![], ddl, size: at(2) })
    }
    fn all_columns(&mut self, schema: &str) -> Result<Vec<(String, String, String)>> {
        let sql = format!("SELECT table, name, type FROM system.columns WHERE database = {} ORDER BY table, position", text(schema));
        Ok(self.text_rows(&sql)?.into_iter().map(|r| (r[0].clone().unwrap_or_default(), r[1].clone().unwrap_or_default(), r[2].clone().unwrap_or_default())).collect())
    }
    fn query(&mut self, sql: &str) -> Result<Page> {
        self.read(sql)
    }
    fn execute(&mut self, sql: &str) -> Result<Vec<(String, Outcome)>, (Vec<(String, Outcome)>, String, anyhow::Error)> {
        let mut done = Vec::new();
        for stmt in split_statements(sql) {
            let start = Instant::now();
            match self.run(&stmt, &[("mutations_sync", "1")]) {
                Ok(Reply::Rows(p)) => done.push((stmt, Outcome::Rows(p))),
                Ok(Reply::Done { written }) => {
                    let command = command_of(&stmt);
                    let rows = (command == "INSERT").then_some(written).flatten();
                    done.push((stmt, Outcome::Done { command, rows, elapsed: start.elapsed() }));
                }
                Err(e) => return Err((done, stmt, e)),
            }
        }
        Ok(done)
    }
    fn apply(&mut self, statements: &[String]) -> Result<()> {
        // No transactions: statements run in order, and each mutation is waited
        // for so the grid reloads what's really there.
        for (i, s) in statements.iter().enumerate() {
            self.run(s, &[("mutations_sync", "1")]).map_err(|e| {
                let kept = if i == 0 { String::new() } else { format!("\n\nClickHouse has no transactions: the {i} statement(s) before this one were kept.") };
                anyhow!("{e}\n\n{s}{kept}")
            })?;
        }
        Ok(())
    }
    fn explain(&mut self, sql: &str) -> Result<Plan> {
        let body = sql.trim().trim_end_matches(';');
        let text = self.first(&format!("EXPLAIN PLAN indexes = 1, json = 1 {body}"))?.context("No plan returned")?;
        let json: serde_json::Value = serde_json::from_str(&text)?;
        let mut nodes = Vec::new();
        walk(&json[0]["Plan"], 0, &mut nodes);
        Ok(Plan { nodes, summary: "Estimated plan — ClickHouse's EXPLAIN doesn't run the query".into(), advice: None })
    }
}

fn walk(node: &serde_json::Value, depth: usize, out: &mut Vec<PlanNode>) {
    let op = node["Node Type"].as_str().unwrap_or("?").to_string();
    let mut detail = node["Description"].as_str().unwrap_or("").to_string();
    // ReadFromMergeTree describes itself as the table it reads, `db.table`.
    let relation = op.starts_with("ReadFrom").then(|| detail.clone()).filter(|d| !d.is_empty() && !d.contains(' '));
    if relation.is_some() {
        detail.clear();
    }
    // What each index pruned: "PrimaryKey: 12/3000 granules".
    let pruned: Vec<String> = node["Indexes"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|i| {
                    let name = i["Type"].as_str().unwrap_or("Index");
                    match (i["Selected Granules"].as_u64(), i["Initial Granules"].as_u64()) {
                        (Some(s), Some(n)) => format!("{name}: {s}/{n} granules"),
                        _ => name.to_string(),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    if !pruned.is_empty() {
        detail = [detail, pruned.join(", ")].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("  ");
    }
    out.push(PlanNode { depth, operation: op, relation, detail, ..Default::default() });
    for c in node["Plans"].as_array().into_iter().flatten() {
        walk(c, depth + 1, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_json_compact() {
        let body = r#"{"meta":[{"name":"id","type":"UInt64"},{"name":"price","type":"Decimal(10, 2)"},{"name":"tags","type":"Array(String)"},{"name":"n","type":"Nullable(Int32)"}],
            "data":[["18446744073709551615","1.50",["a"],null],["7","2.00",[],3]],"rows":2}"#;
        let p = parse(body);
        assert_eq!(p.columns.len(), 4);
        assert_eq!(p.rows[0], vec![Value::Number("18446744073709551615".into()), Value::Number("1.50".into()), Value::Json(serde_json::json!(["a"])), Value::Null]);
        assert_eq!(p.rows[1][0], Value::Int(7));
        assert_eq!(p.rows[1][3], Value::Int(3));
    }

    #[test]
    fn types_and_enums() {
        assert_eq!(base_type("LowCardinality(Nullable(String))"), "String");
        assert_eq!(enum_values("Nullable(Enum8('a' = 1, 'it\\'s' = 2))"), vec!["a", "it's"]);
        assert_eq!(tidy_error("Code: 60. DB::Exception: Table default.x does not exist. (UNKNOWN_TABLE) (version 24.3.1.1)\n"), "Table default.x does not exist. (UNKNOWN_TABLE)");
        assert_eq!(text("a\\b'c"), "'a\\\\b''c'");
    }
}

/// Against a live server: `TABLES_CH_URL=clickhouse://user:pass@host/db cargo test live -- --ignored --nocapture`.
#[cfg(test)]
mod live {
    use super::*;
    #[test]
    #[ignore]
    fn live() {
        let url = std::env::var("TABLES_CH_URL").expect("TABLES_CH_URL");
        let (c, pass) = config::from_url(&url).unwrap();
        let mut d = ClickHouse::connect(&c, pass).unwrap();
        println!("{}", d.version().unwrap());
        let schemas = d.schemas().unwrap();
        println!("schemas: {schemas:?}");
        let rels = d.relations(&d.default_schema()).unwrap();
        println!("{} relations, first: {:?}", rels.len(), rels.first());
        let t = rels.iter().find(|r| r.kind == RelationKind::Table).unwrap().name.clone();
        let det = d.detail(&d.default_schema(), &t).unwrap();
        println!("{t}: {} cols, pk {:?}, idx {:?}, size {:?}", det.columns.len(), det.columns.iter().filter(|c| c.primary_key).map(|c| &c.name).collect::<Vec<_>>(), det.indexes.iter().map(|i| &i.name).collect::<Vec<_>>(), det.size);
        println!("{}", det.ddl.lines().next().unwrap_or(""));
        let names: Vec<String> = det.columns.iter().map(|c| c.name.clone()).collect();
        let w = grid_where(Engine::ClickHouse, &[], &names, "a");
        let p = d.query(&select_page(Engine::ClickHouse, &d.default_schema(), &t, &w, None, 3, 0)).unwrap();
        println!("page: {} rows, cols {:?}", p.rows.len(), p.columns.iter().take(4).map(|c| format!("{}:{}", c.name, c.type_name)).collect::<Vec<_>>());
        println!("row0: {:?}", p.rows.first().map(|r| r.iter().take(4).collect::<Vec<_>>()));
        let out = d.execute("SELECT 1 AS a, 'x' AS b; SELECT toUInt64(18446744073709551615) AS big").unwrap();
        println!("execute: {} results", out.len());
        let plan = d.explain(&format!("SELECT count() FROM {} WHERE 1", qualified(Engine::ClickHouse, &d.default_schema(), &t))).unwrap();
        for n in &plan.nodes {
            println!("{}{} {:?} {}", "  ".repeat(n.depth), n.operation, n.relation, n.detail);
        }
        let err = d.query("SELECT * FROM no_such_table_here").unwrap_err();
        println!("error: {err}");
        assert!(d.ping());
    }
}
