
# iotactl

This is a ranger-style Rust TUI program. It will, at some point in the future, have non-filesystem based node sources and thus allow for navigation through trees of things that aren't necessarily on the local disk and aren't necessarily really files. 

Avoid reading or acting on information in `ISSUES.md`, `FEATURES.md`, or other markdown files at the root of the repository besides this one except where specifically asked.

## Testing

Run `cargo` tests frequently, e.g. whenever building. The tests in `sh-test` (`sh-test/run_all.sh`) should be run at least upon completing any task (perhaps even more frequently to the extent it's useful and doesn't waste time).

