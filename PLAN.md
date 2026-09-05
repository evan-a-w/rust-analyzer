# Plan: rust-analyzer public API interface view

## Goal

Add a native rust-analyzer-backed way to open a read-only, `cargo public-api`-style public interface view for the Rust module corresponding to the current source file. The default view should be module-scoped, e.g. `src/arith/int.rs` opens an interface for `crate::arith::int`, similar in spirit to opening an OCaml `.mli`.

## Desired behavior

- VS Code command opens a generated public API view for the active Rust file.
- The generated view uses a custom URI scheme owned by rust-analyzer, not an ordinary temp file.
- The default contents are the public API for the module represented by the source file.
- Basic LSP-like behavior in that buffer is native: hover/definition/document symbols should be routed through rust-analyzer using a mapping from rendered text ranges to real Rust items.

## Implementation phases

1. Recon rust-analyzer request/command, virtual document, and VS Code client architecture.
2. Add a server-side public API renderer for a source file's module.
3. Expose the renderer through a rust-analyzer extension request/command.
4. Add VS Code client support to open a virtual read-only document from the server response.
5. Add native providers for the public API URI scheme where feasible in this first pass.
6. Validate with `~/dev/rocks`, especially `src/arith/int.rs`.

## Current status

- Repository located at `./rust-analyzer`.
- Example project available at `~/dev/rocks`.
- `cargo public-api -sss` works in `~/dev/rocks` and provides the target visual style.
- Recon found existing VS Code virtual document patterns (`editors/code/src/commands.ts`) and server custom request plumbing (`crates/rust-analyzer/src/lsp/ext.rs`, `handlers/request.rs`, `main_loop.rs`).
- Implemented a first end-to-end version:
  - `ide::Analysis::public_api` renders module-scoped public API text and source mappings.
  - Server exposes `rust-analyzer/publicApi` returning text, module name, and mapped source locations.
  - VS Code command `rust-analyzer.publicApi` opens a `rust-analyzer-public-api:` virtual document.
  - VS Code registers definition, hover, and document-symbol providers for the virtual document using rust-analyzer-provided mappings.
- Validation passed:
  - `cargo test -p ide public_api -- --nocapture`
  - `cargo check -p ide -p rust-analyzer`
  - `npm run typecheck` in `editors/code` after `npm ci`
- Fresh-context reviewers flagged over/under-reporting issues. Addressed in the implementation:
  - Added `hir::Module::public_scope` so facade modules include public re-exports while private imports remain excluded.
  - Filtered impl blocks so associated items on private receiver types / private traits do not appear as public API.
  - Added rendering coverage for trait associated items and public ADT child surface via limited HIR display with private fields hidden.
  - Added focused tests for reexports, trait items, public enum/field display, and private impl filtering.

## Notes / constraints

- The `cargo public-api` textual format is not valid Rust, so full ordinary rust-analyzer parsing cannot work by pretending it is a `file://` Rust document.
- The right model is a rust-analyzer-owned virtual document with source mappings back to real HIR items.
- First implementation supports a useful subset and is not yet full `cargo public-api` parity. Known gaps:
  - Rendering is HIR-display-based and may differ from `cargo public-api` for private fields, parameter-name omission, blanket/auto/derived impl filtering, and some macro forms.
  - Hover/definition/document-symbol are VS Code providers backed by rust-analyzer mappings, not general server-side LSP support for arbitrary non-file URIs.
  - Reopen persistence is in-memory for the VS Code extension session.
