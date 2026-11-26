# Tables

A native database client built with GPUI, in the same visual language as `../chat`.
The design it implements is `mockups/index.html`.

```sh
cargo run --release                                   # pick or add a connection
cargo run --release -- path/to/file.sqlite            # open a SQLite file directly
cargo run --release -- postgres://user:pass@host/db   # or a Postgres URL
```

Try it on demo data: `scripts/seed-demo.py demo.sqlite`, then `cargo run -- demo.sqlite`.
`scripts/seed-demo.py --sql | psql …` loads the same data into Postgres.

To install on your Mac, `./scripts/install-app.sh` (builds `build/Tables.app` and copies it
to `/Applications`; `TABLES_INSTALL_DIR` overrides the location).

> The app is signed with a code-signing certificate from your keychain so macOS keeps its
> Keychain grants (saved passwords) across updates. If you have more than one, run
> `./scripts/install-app.sh --choose-identity` once to pick which to use; the choice is saved
> to `.signing-identity` (gitignored). `TABLES_SIGN_IDENTITY` overrides it for a single build.

## What's here

- **Connections**: PostgreSQL (TLS: off / prefer / require / require-any-cert) and SQLite.
  Each is tagged development, staging or production. Production shows as a red stripe and
  chip, and every write asks first: grid commits, and SQL that isn't read-only.
  Passwords live in the macOS Keychain; everything else lives in
  `~/Library/Application Support/Tables/store.json` (`TABLES_HOME` overrides the location).
- **Sidebar**: pinned tables, tables, views, functions, saved queries, with filter (⌘F) and
  row counts. Right-click for structure, a query on the table, pin, or a drafted TRUNCATE/DROP
  (drafted into a SQL tab, never run directly).
- **Tabs**: single-click opens a preview tab (italic) that the next single-click replaces;
  double-click or editing keeps it. ⌘1–9, ⌃Tab to switch, ⌘W to close.
- **Table tab**:
  - Virtualized grid with row numbers that stay put while you scroll sideways, resizable
    columns, and values drawn by type (NULL, booleans, JSON, timestamps, enums with
    coloured dots, foreign-key jump arrows).
  - Sort by clicking a header. Without a sort, rows follow the primary key.
  - Filter builder that shows the `WHERE` it generates, plus a search across all columns.
    Paging with a choice of limit.
  - Editing: type or press ↵ on a cell, ⌘⌫ deletes a row, + Row inserts one. Changes are
    tinted until committed, and Review SQL shows the exact `BEGIN … COMMIT` that will run.
    ⌘S commits in one transaction; nothing is kept if any statement fails.
  - Structure view: columns, indexes (unused ones flagged), referenced-by, DDL.
  - Right-click a cell: copy value, row as JSON or INSERT, filter by value, open the
    referenced row, set NULL, duplicate, delete.
  - Export: copy as CSV, JSON or INSERTs, or save a CSV.
- **SQL tab**:
  - Tree-sitter highlighting, with completion that reads aliases (`o.` lists the columns of
    `orders o`).
  - ⌘↵ runs the selection or the statement under the cursor, ⌘⇧↵ runs everything, and
    ⌃C cancels (Postgres).
  - One result tab per result set, a message log, and Explain (⌘E). Explain runs
    EXPLAIN ANALYZE for reads, shows self time per node, and suggests an index for the
    worst sequential scan.
  - History, and saved queries (⌘S; a leading `-- comment` names the query).
- **Row detail panel** (⌘.): a form for the selected row. Toggles for booleans, menus for
  enums, a JSON editor, and FK links. Edits go into the same pending changes as the grid.
  Below the fields, "Referenced by" opens related rows.
- **Palette** (⌘K): tables, saved queries, commands, connections.

## Layout

| Path | What it does |
| --- | --- |
| `src/db/` | Engine-neutral types, SQL generation, and the drivers (`postgres.rs`, `sqlite.rs`). Drivers block and run on background threads behind a mutex. |
| `src/session.rs` | An open connection plus its schema cache (tables, columns, details) |
| `src/grid.rs` | The data grid and its pending-change tracking |
| `src/table_view.rs`, `src/sql_view.rs` | The two kinds of tab |
| `src/completion.rs` | SQL completion for gpui-component's code editor |
| `src/workspace/` | Window chrome, sidebar, detail panel, connection form, overlays |
| `src/theme.rs`, `src/menu.rs` | Shared with `../chat` |

## Not yet

MySQL, SSH tunnels, editing query results in place (open the table instead), hiding
columns, and multi-cell selection.
