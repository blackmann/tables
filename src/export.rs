//! Turning grid contents into CSV, JSON or INSERT statements.
use crate::db::{Change, Engine, Value};

pub fn csv(columns: &[String], rows: &[Vec<Value>]) -> String {
    let field = |s: &str| {
        if s.contains([',', '"', '\n', '\r']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() }
    };
    let mut out = columns.iter().map(|c| field(c)).collect::<Vec<_>>().join(",");
    out.push('\n');
    for r in rows {
        let line = r.iter().map(|v| if v.is_null() { String::new() } else { field(&v.display()) }).collect::<Vec<_>>().join(",");
        out += &line;
        out.push('\n');
    }
    out
}

pub fn json(columns: &[String], rows: &[Vec<Value>]) -> String {
    let arr: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            let mut m = serde_json::Map::new();
            for (c, v) in columns.iter().zip(r) {
                m.insert(c.clone(), to_json(v));
            }
            serde_json::Value::Object(m)
        })
        .collect();
    serde_json::to_string_pretty(&arr).unwrap_or_default()
}

pub fn to_json(v: &Value) -> serde_json::Value {
    match v {
        Value::Null | Value::Default => serde_json::Value::Null,
        Value::Bool(b) => (*b).into(),
        Value::Int(i) => (*i).into(),
        Value::Float(f) => serde_json::Number::from_f64(*f).map(serde_json::Value::Number).unwrap_or(serde_json::Value::Null),
        Value::Json(j) => j.clone(),
        other => other.display().into(),
    }
}

pub fn inserts(engine: Engine, table: &str, columns: &[String], rows: &[Vec<Value>]) -> String {
    rows.iter()
        .map(|r| Change::Insert { values: columns.iter().cloned().zip(r.iter().cloned()).collect() }.to_sql(engine, table))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quotes_csv_fields() {
        let s = csv(&["a".into(), "b".into()], &[vec![Value::Text("x,y".into()), Value::Null]]);
        assert_eq!(s, "a,b\n\"x,y\",\n");
    }
}
