# sqlite-thin

A small, safe SQLite wrapper whose API is a subset of
[rusqlite](https://docs.rs/rusqlite)'s.

## Why

`libsqlite3-sys` declares `links = "sqlite3"`, so a build can contain only one
version of it. Each rusqlite release pins a single `libsqlite3-sys` version,
so every crate in a dependency graph that uses rusqlite must agree on one
rusqlite release.

This crate declares the SQLite C functions it calls by hand (`src/sys.rs`);
they have been stable since 2014. It uses `libsqlite3-sys` only to build and
link SQLite, so it accepts any version (`>=0.28`) and adopts whichever one the
rest of the graph selects. It coexists with rusqlite: both share the one
`libsqlite3-sys` and the one copy of SQLite in the process.

The constants in `sqlite_thin::ffi` are this crate's own; a test checks them
against `libsqlite3-sys`.

## Scope

Included: connections, statements, positional and named parameters, typed
rows, a prepared-statement cache, transactions, pragmas, and `ToSql`/`FromSql`
conversions for common types.

Not included: user functions, hooks, virtual tables, backups, incremental
blob I/O, savepoints, and `interrupt`.

## Differences from rusqlite

- `Rows::next` returns `Option<Row<'_>>` rather than `Option<&Row<'_>>`.
  Closures passed to `query_row` and `query_map` are unchanged.
- `prepare` rejects SQL holding more than one statement with
  `Error::MultipleStatement`, as newer rusqlite releases do. Use
  `execute_batch` for scripts.
- `Error::SqliteFailure` carries `sqlite_thin::SqliteError` instead of
  `ffi::Error`. Both expose `code` and `extended_code`.
- `sqlite_thin::ffi` holds constants only, not the `libsqlite3-sys` crate.
- Like rusqlite, new connections wait up to five seconds for locks.

## Features

- `bundled`: compile SQLite from source.
- `bundled-sqlcipher`: compile SQLCipher from source (enables `PRAGMA key`).

With none of these, the system SQLite is linked. Cargo features are additive
across the graph, so any crate enabling `bundled` on `libsqlite3-sys` wins.

## Safety

All `unsafe` code lives in `src/raw.rs` and the declarations in
`src/sys.rs`, and every unsafe block has a `SAFETY` comment. The crate denies `unsafe_code` everywhere else. Column values borrow
the statement they were read from, so the borrow checker prevents using a
value after the next `step`, `reset`, or `finalize`.
