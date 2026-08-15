
# Issues

Known or potential issues to look into (actual bugs or usability concerns).

## Avoid syntax highlighting for very large files

We can perhaps impose a time limit on the execution of treesitter, and if it takes too long, cancel the task, emit a warning and display plain text.

## Avoid rendering preview for very very large files

Confirm we have protections not to preview really big files, even if they're entirely text.

## File identification

I suspect the file identification logic is not smart enough to handle anything beyond basic identification based on file extension. We'd want support for identification also on the basis of:

 - Shebang lines
 - Special filenames like `Dockerfile` and such

## Look into how the color scheme renders in different terminals

Can we try to make it always be pretty by default, perhaps using true colour?

But make it allow for different terminal capabilities and allow for user customisation?

## Show minor visual feedback for toggles in the status bar

Show whether wrapping or hidden files viewing is toggled on or off.

## Test error handling behaviour when intermediate directories that are being viewed are removed or renamed

## It seems like we run into trouble when viewing certain (binary, and perhaps misidentified as text?) files

.git/COMMIT_EDITMSG for example

Leads to jank terminal output

