//! SQL completion from the live schema: tables after FROM/JOIN, columns after
//! `alias.`, and keywords and functions otherwise. Aliases are read from the
//! FROM and JOIN clauses of the whole script.
use crate::session::Session;
use anyhow::Result;
use gpui::{Context, Entity, Task, Window};
use gpui_component::input::{CompletionProvider, InputState};
use gpui_component::{Rope, RopeExt};
use lsp_types::{CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit, TextEdit};
use std::collections::HashMap;

pub const KEYWORDS: &[&str] = &[
    "select", "from", "where", "and", "or", "not", "in", "is", "null", "join", "left", "right", "inner", "outer", "full", "cross", "on", "as", "group", "by", "order",
    "having", "limit", "offset", "distinct", "union", "all", "insert", "into", "values", "update", "set", "delete", "returning", "with", "case", "when", "then",
    "else", "end", "asc", "desc", "between", "like", "ilike", "exists", "create", "table", "alter", "drop", "index", "primary", "key", "references", "default",
    "interval", "true", "false", "explain", "analyze", "over", "partition", "filter", "lateral", "begin", "commit", "rollback", "truncate", "view", "using",
    "nulls", "first", "last", "unique", "constraint", "foreign", "check", "cascade", "column", "add", "rename", "to", "if", "only",
];

pub const FUNCTIONS: &[&str] = &[
    "count", "sum", "avg", "min", "max", "now", "date_trunc", "lower", "upper", "coalesce", "round", "extract", "to_char", "jsonb_build_object", "json_agg",
    "jsonb_agg", "array_agg", "string_agg", "row_number", "rank", "dense_rank", "lag", "lead", "greatest", "least", "nullif", "length", "substring", "trim",
    "replace", "concat", "cast", "abs", "ceil", "floor", "random", "gen_random_uuid", "date", "datetime", "strftime", "ifnull", "julianday", "json_extract",
];

pub struct SqlCompletion {
    pub session: Entity<Session>,
}

/// Table names a script mentions, by the alias or name it's referred to as.
pub fn aliases(sql: &str, tables: &[String]) -> HashMap<String, String> {
    let words: Vec<String> = sql
        .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.' || c == '"'))
        .filter(|w| !w.is_empty())
        .map(|w| w.trim_matches('"').to_string())
        .collect();
    let mut map = HashMap::new();
    let stop = ["on", "where", "join", "left", "right", "inner", "outer", "group", "order", "limit", "using", "cross", "full", "natural", "set", "values", "union", "having", "window"];
    for i in 0..words.len() {
        if !matches!(words[i].to_ascii_lowercase().as_str(), "from" | "join" | "update" | "into") {
            continue;
        }
        let Some(name) = words.get(i + 1) else { continue };
        let bare = name.rsplit('.').next().unwrap_or(name).to_string();
        let Some(table) = tables.iter().find(|t| t.eq_ignore_ascii_case(&bare)) else { continue };
        map.insert(table.to_ascii_lowercase(), table.clone());
        let mut k = i + 2;
        if words.get(k).is_some_and(|w| w.eq_ignore_ascii_case("as")) {
            k += 1;
        }
        if let Some(alias) = words.get(k)
            && !stop.contains(&alias.to_ascii_lowercase().as_str())
            && !KEYWORDS.contains(&alias.to_ascii_lowercase().as_str())
        {
            map.insert(alias.to_ascii_lowercase(), table.clone());
        }
    }
    map
}

#[derive(Debug, PartialEq)]
pub struct Suggestion {
    pub label: String,
    pub detail: String,
    pub kind: CompletionItemKind,
    pub insert: String,
}

/// What to offer at `offset` in `sql`, and where the word being completed starts.
pub fn suggest(sql: &str, offset: usize, tables: &[(String, Option<i64>)], columns: &HashMap<String, Vec<(String, String)>>) -> Option<(usize, Vec<Suggestion>)> {
    let before = &sql[..offset.min(sql.len())];
    let line = before.rsplit('\n').next().unwrap_or("");
    // Not inside a comment or a string.
    if line.contains("--") || line.matches('\'').count() % 2 == 1 {
        return None;
    }
    let word_start = before.rfind(|c: char| !(c.is_alphanumeric() || c == '_')).map_or(0, |i| i + before[i..].chars().next().map_or(1, |c| c.len_utf8()));
    let prefix = &before[word_start..];
    let lower = prefix.to_ascii_lowercase();
    let names: Vec<String> = tables.iter().map(|t| t.0.clone()).collect();
    let map = aliases(sql, &names);
    let mut out = Vec::new();

    if before[..word_start].ends_with('.') {
        // alias.column
        let qual_end = word_start - 1;
        let qual_start = before[..qual_end].rfind(|c: char| !(c.is_alphanumeric() || c == '_' || c == '"')).map_or(0, |i| i + 1);
        let qual = before[qual_start..qual_end].trim_matches('"').to_ascii_lowercase();
        let table = map.get(&qual).cloned().or_else(|| names.iter().find(|n| n.eq_ignore_ascii_case(&qual)).cloned())?;
        for (c, ty) in columns.get(&table).into_iter().flatten() {
            if c.to_ascii_lowercase().starts_with(&lower) {
                out.push(Suggestion { label: c.clone(), detail: ty.clone(), kind: CompletionItemKind::FIELD, insert: c.clone() });
            }
        }
        return (!out.is_empty()).then_some((word_start, out));
    }

    if prefix.is_empty() {
        return None;
    }
    let prev = before[..word_start].split_whitespace().last().unwrap_or("").to_ascii_lowercase();
    let wants_table = matches!(prev.as_str(), "from" | "join" | "into" | "update" | "table");
    for (t, rows) in tables {
        if t.to_ascii_lowercase().starts_with(&lower) {
            let detail = rows.map(|n| format!("{} rows", crate::db::compact(n as f64))).unwrap_or_else(|| "table".into());
            out.push(Suggestion { label: t.clone(), detail, kind: CompletionItemKind::CLASS, insert: t.clone() });
        }
    }
    if !wants_table {
        let mut in_scope: Vec<&String> = map.values().collect();
        in_scope.sort();
        in_scope.dedup();
        for t in in_scope {
            for (c, ty) in columns.get(t).into_iter().flatten() {
                if c.to_ascii_lowercase().starts_with(&lower) && !out.iter().any(|s| s.label == *c) {
                    out.insert(0, Suggestion { label: c.clone(), detail: format!("{t} · {ty}"), kind: CompletionItemKind::FIELD, insert: c.clone() });
                }
            }
        }
        if lower.len() >= 2 {
            for f in FUNCTIONS.iter().filter(|f| f.starts_with(&lower)) {
                out.push(Suggestion { label: f.to_string(), detail: "function".into(), kind: CompletionItemKind::FUNCTION, insert: format!("{f}(") });
            }
            for k in KEYWORDS.iter().filter(|k| k.starts_with(&lower) && **k != lower) {
                out.push(Suggestion { label: k.to_ascii_uppercase(), detail: "keyword".into(), kind: CompletionItemKind::KEYWORD, insert: k.to_ascii_uppercase() });
            }
        }
    }
    out.retain(|s| !s.label.eq_ignore_ascii_case(prefix));
    out.truncate(40);
    (!out.is_empty()).then_some((word_start, out))
}

impl CompletionProvider for SqlCompletion {
    fn completions(&self, text: &Rope, offset: usize, _: CompletionContext, _: &mut Window, cx: &mut Context<InputState>) -> Task<Result<CompletionResponse>> {
        let sql = text.to_string();
        let s = self.session.read(cx);
        let tables: Vec<(String, Option<i64>)> = s.relations.iter().map(|r| (r.name.clone(), r.rows)).collect();
        let Some((start, items)) = suggest(&sql, offset, &tables, &s.columns) else {
            return Task::ready(Ok(CompletionResponse::Array(vec![])));
        };
        let range = lsp_types::Range { start: text.offset_to_position(start), end: text.offset_to_position(offset) };
        // The menu highlights `filter_text.len()` bytes of each label: keep that to
        // the part actually typed, which every label starts with.
        let typed = sql[start..offset.min(sql.len())].chars().count();
        let items = items
            .into_iter()
            .enumerate()
            .map(|(i, s)| CompletionItem {
                filter_text: Some(s.label.chars().take(typed).collect()),
                label: s.label,
                kind: Some(s.kind),
                detail: Some(s.detail),
                sort_text: Some(format!("{i:04}")),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit { range, new_text: s.insert })),
                ..Default::default()
            })
            .collect();
        Task::ready(Ok(CompletionResponse::Array(items)))
    }

    fn is_completion_trigger(&self, _offset: usize, new_text: &str, _: &mut Context<InputState>) -> bool {
        new_text.chars().count() == 1 && new_text.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.')
    }
}

/// Upper-case keywords outside strings, comments and quoted names.
pub fn format_keywords(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let chars: Vec<char> = sql.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' || c == '"' {
            let q = c;
            out.push(c);
            i += 1;
            while i < chars.len() {
                out.push(chars[i]);
                i += 1;
                if chars[i - 1] == q {
                    break;
                }
            }
        } else if c == '-' && chars.get(i + 1) == Some(&'-') {
            while i < chars.len() && chars[i] != '\n' {
                out.push(chars[i]);
                i += 1;
            }
        } else if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let w: String = chars[start..i].iter().collect();
            let lw = w.to_ascii_lowercase();
            let before_dot = start > 0 && chars[start - 1] == '.';
            if !before_dot && (KEYWORDS.contains(&lw.as_str()) || (FUNCTIONS.contains(&lw.as_str()) && chars.get(i) == Some(&'('))) {
                out += &if KEYWORDS.contains(&lw.as_str()) { w.to_ascii_uppercase() } else { lw };
            } else {
                out += &w;
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> (Vec<(String, Option<i64>)>, HashMap<String, Vec<(String, String)>>) {
        let tables = vec![("users".to_string(), Some(10)), ("orders".to_string(), None)];
        let mut cols = HashMap::new();
        cols.insert("users".to_string(), vec![("id".to_string(), "int8".to_string()), ("email".to_string(), "text".to_string())]);
        cols.insert("orders".to_string(), vec![("id".to_string(), "int8".to_string()), ("status".to_string(), "text".to_string())]);
        (tables, cols)
    }

    #[test]
    fn columns_after_alias() {
        let (t, c) = schema();
        let sql = "select * from users u join orders o on o.";
        let (start, s) = suggest(sql, sql.len(), &t, &c).unwrap();
        assert_eq!(start, sql.len());
        assert_eq!(s.iter().map(|s| s.label.as_str()).collect::<Vec<_>>(), vec!["id", "status"]);
        let sql = "select u.em from users u";
        let (_, s) = suggest(sql, 11, &t, &c).unwrap();
        assert_eq!(s[0].label, "email");
    }

    #[test]
    fn tables_after_from() {
        let (t, c) = schema();
        let sql = "select * from or";
        let (start, s) = suggest(sql, sql.len(), &t, &c).unwrap();
        assert_eq!((start, s[0].label.as_str()), (14, "orders"));
    }

    #[test]
    fn nothing_in_strings() {
        let (t, c) = schema();
        let sql = "select 'us";
        assert!(suggest(sql, sql.len(), &t, &c).is_none());
    }

    #[test]
    fn uppercases_keywords_only() {
        assert_eq!(format_keywords("select count(*) from t where name = 'select' -- from"), "SELECT count(*) FROM t WHERE name = 'select' -- from");
    }
}
