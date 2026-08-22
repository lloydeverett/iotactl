
# iotactl

This is a ranger-style Rust TUI program, designed to navigate arbitrary hierarchies of nodes, including file hierarchies but also sources that don't have typical filesystem semantics.

Avoid reading or acting on information in `ISSUES.md`, `FEATURES.md`, or other markdown files at the root of the repository besides this one except where specifically asked.

## Testing

Run `cargo` tests frequently, e.g. whenever building. The tests in `sh-test` (`sh-test/run_all.sh`) should be run at least upon completing any task (perhaps even more frequently to the extent it's useful and doesn't waste time).

## Documentation style

Doc comments and other prose in this repo tend to run long. Keep new or edited ones short and focused on what a reader needs *right now* to use the item correctly:

- State what the item does and any real constraint or gotcha, not the history of how it got that way. A doc comment isn't a changelog — don't narrate a prior design, why an earlier approach fell short, or what changed and when. It's fine to mention a genuine drawback of a naive/obvious approach if it heads off a mistake (e.g. "a plain X would Y, so this does Z instead"), but keep it brief.
- Don't note that a function or field "isn't used/invoked anywhere yet". That's a fact about the codebase's current state, not about the interface — it goes stale the moment a caller shows up, and `grep`/`cargo check` answer the question directly for anyone who needs it.
- Don't over-describe specific existing consumers. An interface's docs shouldn't read like they know everything that currently implements or calls it — that couples the doc to code that's free to change independently. An occasional example or "see X for the pattern" pointer is fine when it genuinely helps, but don't enumerate every current caller/implementor or explain their individual reasoning.

## Writing a new node source

Work through this checklist when adding a new `NodeSource` implementation (a new `NodeSourceType`):

- **Provide a manual page.** Contribute a `ManualPage` via `NodeSourceType::manual_page` describing what the source is and how it's scoped (root, toggles, commands) — see `fs/docs.rs`'s `MANUAL_PAGE` for the pattern, and `manual::ManualSource`'s module/doc comments for how contributed pages get spliced into the manual's tree. Write the text in ASD-STE100 style (short sentences, one idea per sentence, plain words) — see `fs/docs.rs`'s module doc comment and `manual::ABOUT_MANUAL` for examples. A `None` `manual_page` is only correct for `manual` itself, whose own pages already are the manual. Manual text constants should be defined in a manual.rs file in your module.
- **Keep nontrivial work off the render thread.** `async fn` alone doesn't make a call non-blocking — see `NodeSource`'s trait-level doc comment ("Do real work off the render thread"). Any real work — a directory listing, a file read, a syntax highlight, or any other computationally or IO-heavy operation that doesn't hit a genuine yield point on its own — must run inside `tokio::task::spawn_blocking`, with the `JoinHandle` `.await`ed and a panic mapped to a visible error rather than left to unwind into the caller. See `fs::FsSource::read_dir`/`preview_tui` for the pattern.
- **Let the source be rooted at a node of the caller's choosing**, not always its outermost/default node. Scope the starting point at construction, the way `fs::FsSource::new`'s `root` parameter pins a real directory or `manual::ManualSource::new`'s `root` parameter pins a page (e.g. `manual://filesystem` roots the manual at the Filesystem topic instead of the top level). `id == []` passed to any `NodeSource` method should always mean "this instance's own configured root", never some fixed absolute root.
- **Stream functionality should be offered based on the intrinsic capabilities of the underlying store.** If there underlying streams are by nature not random access, for instance, use the simulated random access implemented in `streams.rs`. (Or, if that seems inappropriate, then there is always the option of not implementing random access at all. Consider confirming with the user.)
- **Use pipes when it makes sense.** Paths should index into nodes in which the node source
specializes. So, a `zip://foobar` tells the ZIP node source to reach for `foobar` *inside the zip*,
but a pipe is used to tell the source which ZIP file to read. Notice that this pattern abstracts
away dependence on how and where the underlying data is stored and ensures there is a clear
distinction between an index into the node tree itself (`./foobar` in this case) and the location
of the node store itself (the ZIP file, in this case).

