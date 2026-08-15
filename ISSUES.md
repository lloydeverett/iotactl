
# Issues

Known or potential issues to look into (actual bugs or usability concerns).

## Look into how the color scheme renders in different terminals

Can we try to make it always be pretty by default, perhaps using true colour?

But make it allow for different terminal capabilities and allow for user customisation?

## Show minor visual feedback for toggles in the status bar

Show whether wrapping or hidden files viewing is toggled on or off.

## More vim-like pager motions in the preview

Currently we just support g/GG/j/k. Support page navigation too.

## Test error handling behaviour when intermediate directories that are being viewed are removed or renamed

## It seems like we run into trouble when viewing certain (binary, and perhaps misidentified as text?) files

.git/COMMIT_EDITMSG for example

Leads to jank terminal output

