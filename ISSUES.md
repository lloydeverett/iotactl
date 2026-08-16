
# Issues

Known or potential issues to look into (actual bugs or usability concerns).

## Fix without agent: entering empty dir still has weird graphical glitches here, check the logic

Basically only visible if you start the program in debug mode without a multiplexer like vim, then try it a few times with an empty directory.

## Avoid syntax highlighting for very large files

We can perhaps impose a time limit on the execution of treesitter, and if it takes too long, cancel the task, emit a warning and display plain text.

## How does syntax highlighting work on partial file previews?

## Is there sufficient indication to the user when a file has been truncated due to it being too large?

## Avoid rendering preview for very very large files

Confirm we have protections not to preview really big files, even if they're entirely text.

## Look into how the color scheme renders in different terminals

Can we try to make it always be pretty by default, perhaps using true colour?

But make it allow for different terminal capabilities and allow for user customisation?

## Test error handling behaviour when intermediate directories that are being viewed are removed or renamed

