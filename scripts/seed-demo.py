#!/usr/bin/env python3
"""Build a demo database shaped like the design mockup.

    scripts/seed-demo.py demo.sqlite          # SQLite file
    scripts/seed-demo.py --sql > demo.sql     # portable SQL, e.g. for `psql -f`
"""
import json
import re
import random
import sqlite3
import sys
from datetime import datetime, timedelta, timezone

R = random.Random(42)
NOW = datetime(2026, 11, 24, 9, 41, 12, tzinfo=timezone.utc)
FIRST = "Ama Kofi Lena Marcus Priya Tomas Yuki Noah Esi Ibrahim Sofia Daniel Aisha Chen Olivia Kwame Mateo Hana Jonas Zara Felix Nia Arjun Maya Elena Kwesi".split()
LAST = "Mensah Okafor Schmidt Rivera Patel Nakamura Andersen Boateng Haddad Rossi Kim Novak Asante Silva Dubois Owusu Fischer Tanaka Larsen Costa Adjei".split()
COMPANIES = ["Northwind", "Globex", "Initech", "Umbrella Labs", "Hooli", "Vandelay Industries", "Stark Freight", "Wayne Analytics", "Acme Retail",
             "Pied Piper", "Soylent Foods", "Tyrell Systems", "Cyberdyne", "Oscorp Bio", "Wonka Supply", "Monarch Media", "Aperture", "Black Mesa"]
CITIES = [("Accra", "GH"), ("Berlin", "DE"), ("Lisbon", "PT"), ("Toronto", "CA"), ("Austin", "US"), ("Lagos", "NG"), ("Tokyo", "JP"), ("London", "GB")]
PRODUCTS = ["Ceramic Pour-Over", "Linen Apron", "Walnut Desk Tray", "Merino Throw", "Cast Iron Skillet", "Brass Desk Lamp", "Oak Cutting Board",
            "Stoneware Mug", "Wool Felt Coaster", "Copper Kettle", "Leather Notebook", "Bamboo Steamer"]

SCHEMA = """
CREATE TABLE customers (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  domain TEXT UNIQUE,
  plan TEXT NOT NULL DEFAULT 'free' CHECK (plan IN ('free','pro','enterprise')),
  seats INTEGER NOT NULL DEFAULT 1,
  mrr NUMERIC(10,2) NOT NULL DEFAULT 0,
  country CHAR(2),
  created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE TABLE users (
  id INTEGER PRIMARY KEY,
  email TEXT NOT NULL UNIQUE,
  full_name TEXT NOT NULL,
  role TEXT NOT NULL DEFAULT 'member',
  customer_id INTEGER REFERENCES customers(id),
  is_verified BOOLEAN NOT NULL DEFAULT FALSE,
  last_login_at TIMESTAMP,
  metadata JSON NOT NULL DEFAULT '{}',
  created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX idx_users_customer_id ON users(customer_id);
CREATE TABLE products (
  id INTEGER PRIMARY KEY,
  sku TEXT NOT NULL UNIQUE,
  name TEXT NOT NULL,
  price NUMERIC(10,2) NOT NULL,
  stock INTEGER NOT NULL DEFAULT 0,
  active BOOLEAN NOT NULL DEFAULT TRUE,
  created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE TABLE orders (
  id INTEGER PRIMARY KEY,
  user_id INTEGER NOT NULL REFERENCES users(id),
  status TEXT NOT NULL DEFAULT 'pending',
  total_cents INTEGER NOT NULL,
  currency CHAR(3) NOT NULL DEFAULT 'USD',
  shipping_address JSON,
  placed_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
  notes TEXT
);
CREATE INDEX idx_orders_user_id ON orders(user_id);
CREATE TABLE order_items (
  id INTEGER PRIMARY KEY,
  order_id INTEGER NOT NULL REFERENCES orders(id) ON DELETE CASCADE,
  product_id INTEGER NOT NULL REFERENCES products(id),
  quantity INTEGER NOT NULL DEFAULT 1,
  unit_price NUMERIC(10,2) NOT NULL
);
CREATE TABLE invoices (
  id INTEGER PRIMARY KEY,
  number TEXT NOT NULL UNIQUE,
  customer_id INTEGER NOT NULL REFERENCES customers(id),
  amount NUMERIC(10,2) NOT NULL,
  status TEXT NOT NULL DEFAULT 'open',
  due_at DATE,
  paid_at TIMESTAMP
);
CREATE TABLE audit_log (
  id INTEGER PRIMARY KEY,
  actor_id INTEGER REFERENCES users(id),
  action TEXT NOT NULL,
  target TEXT,
  diff JSON,
  at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE VIEW paying_customers AS
  SELECT c.id, c.name, c.plan, c.seats, c.mrr FROM customers c WHERE c.plan <> 'free';
CREATE VIEW monthly_revenue AS
  SELECT substr(placed_at, 1, 7) AS month, count(*) AS orders, sum(total_cents) / 100.0 AS revenue
  FROM orders WHERE status IN ('paid','shipped') GROUP BY 1 ORDER BY 1 DESC;
"""


def ts(d):
    return d.strftime("%Y-%m-%d %H:%M:%S")


def ascii_(s):
    return s.lower().replace(" ", "")


def rows():
    out = {}
    out["customers"] = [
        (200 + i, name, ascii_(name).replace("&", "") + R.choice([".com", ".io", ".co"]), R.choice(["free", "free", "pro", "pro", "pro", "enterprise"]),
         R.choice([1, 3, 5, 12, 25, 40, 120]), round(R.randint(0, 90) * 25 + R.random(), 2), R.choice(CITIES)[1], ts(NOW - timedelta(days=400 - i * 20)))
        for i, name in enumerate(COMPANIES, start=1)
    ]
    users, seen = [], set()
    for i in range(1, 241):
        f, l = R.choice(FIRST), R.choice(LAST)
        email = f"{f.lower()}.{l.lower()}@{R.choice(['gmail.com', 'northwind.io', 'globex.com', 'proton.me', 'hooli.xyz'])}"
        while email in seen:
            email = email.replace("@", f"{R.randint(1, 99)}@")
        seen.add(email)
        meta = {"locale": R.choice(["en-GB", "en-US", "de-DE", "pt-PT", "ja-JP"]), "theme": R.choice(["dark", "light", "system"]), "beta": R.random() < 0.3}
        if R.random() < 0.4:
            meta["onboarding"] = {"step": R.randint(1, 5), "done": R.random() < 0.5}
        last = None if R.random() < 0.18 else ts(NOW - timedelta(hours=R.randint(1, 720)))
        users.append((1000 + i, email, f"{f} {l}", R.choice(["admin", "member", "member", "member", "viewer"]), R.randint(201, 200 + len(COMPANIES)),
                      R.random() < 0.82, last, json.dumps(meta), ts(NOW - timedelta(hours=(241 - i) * 9))))
    out["users"] = users
    out["products"] = [
        (500 + i, f"SKU-{R.choice(['KT', 'HM', 'OF', 'TX'])}-{R.randint(1000, 9999)}-{i}", name, round(R.randint(6, 180) + R.choice([0, 0.5, 0.99]), 2),
         0 if R.random() < 0.1 else R.randint(1, 420), R.random() < 0.88, ts(NOW - timedelta(days=300 - i * 10)))
        for i, name in enumerate(PRODUCTS, start=1)
    ]
    orders, items = [], []
    for i in range(1, 1201):
        city = R.choice(CITIES)
        addr = {"line1": f"{R.randint(1, 240)} {R.choice(['Oak', 'Mill', 'Harbour', 'Station'])} {R.choice(['St', 'Rd', 'Ave'])}", "city": city[0], "country": city[1]}
        oid = 88400 + i
        orders.append((oid, R.randint(1001, 1240), R.choice(["paid", "paid", "paid", "shipped", "shipped", "pending", "refunded", "cancelled"]),
                       (R.randint(8, 400)) * 100 + R.choice([0, 50, 95, 99]), R.choice(["USD", "USD", "EUR", "GBP", "GHS"]), json.dumps(addr),
                       ts(NOW - timedelta(minutes=(1201 - i) * 47)), None if R.random() < 0.8 else R.choice(["Leave at reception", "Gift wrap please", "Fragile"])))
        for _ in range(R.randint(1, 3)):
            items.append((len(items) + 1, oid, R.randint(501, 500 + len(PRODUCTS)), R.choice([1, 1, 2, 3]), round(R.randint(6, 180) + 0.99, 2)))
    out["orders"], out["order_items"] = orders, items
    out["invoices"] = [
        (40100 + i, f"INV-2026-{4100 + i:06d}", R.randint(201, 200 + len(COMPANIES)), round(R.randint(1, 120) * 25 + 49, 2),
         R.choice(["paid", "paid", "open", "void"]), (NOW - timedelta(days=R.randint(-20, 120))).strftime("%Y-%m-%d"),
         None if R.random() < 0.3 else ts(NOW - timedelta(days=R.randint(1, 60))))
        for i in range(1, 301)
    ]
    out["audit_log"] = [
        (i, None if R.random() < 0.15 else R.randint(1001, 1240), R.choice(["user.update", "order.refund", "invoice.void", "user.sign_in", "plan.change"]),
         f"{R.choice(['users', 'orders', 'invoices'])}:{R.randint(1, 90000)}", json.dumps({"before": {"status": "open"}, "after": {"status": R.choice(["paid", "void"])}}),
         ts(NOW - timedelta(minutes=(601 - i) * 13)))
        for i in range(1, 601)
    ]
    return out


def literal(v):
    if v is None:
        return "NULL"
    if isinstance(v, bool):
        return "TRUE" if v else "FALSE"
    if isinstance(v, (int, float)):
        return str(v)
    return "'" + str(v).replace("'", "''") + "'"


def main():
    data = rows()
    if sys.argv[1:] == ["--sql"]:
        pg = SCHEMA.replace("INTEGER PRIMARY KEY", "BIGINT GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY").replace("substr(placed_at, 1, 7)", "to_char(placed_at, 'YYYY-MM')")
        pg = re.sub(r"\bJSON\b", "JSONB", pg)
        print(re.sub(r"\bTIMESTAMP\b", "TIMESTAMPTZ", pg))
        for table, rs in data.items():
            for r in rs:
                print(f"INSERT INTO {table} VALUES ({', '.join(literal(v) for v in r)});")
            # Identity columns continue after the seeded ids.
            print(f"SELECT setval(pg_get_serial_sequence('{table}', 'id'), (SELECT max(id) FROM {table}));")
        return
    path = sys.argv[1] if len(sys.argv) > 1 else "demo.sqlite"
    db = sqlite3.connect(path)
    db.executescript(SCHEMA)
    for table, rs in data.items():
        db.executemany(f"INSERT INTO {table} VALUES ({','.join('?' * len(rs[0]))})", rs)
    db.commit()
    print(f"Wrote {path}")


if __name__ == "__main__":
    main()
