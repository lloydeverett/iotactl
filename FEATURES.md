
# Features

For features we'd like to have but we need to (1) think about where they fit in the UX and (2) want to ponder how best to introduce these architecturally so as not to overcomplicate the code.

## Shift-(motion)

Allow scroll motions in the "alternate" pane: the preview pane when our cursor is currently at a file in the column immediately to the left of the preview, or the directory pane immediately to the left of the preview pane when the preview pane is selected.

This would permit switching through files while the preview pane stays selected on the one hand and would permit scrolling the preview while the directory pane stays selected on the other hand.

## Toggle management

Putting all these toggles in the status bar / assigning keybindings is going to become untenable, so find a way to manage these. Maybe should just be commands and integrate with whatever command palette we end up implementing?

### Rendering mode

It'd be nice to have the option to render markdown by having a feature to conceal formatting characters. Maybe similar type of thing for JSON.

### Line number column for file previews

### Smooth scrolling

When using Ctrl+D, Ctrl+U, page up, page down

## Command-line flags

Also for toggling all of the toggles above through this mechanism, too!

### --tab-width

Customise tab width.

### --nerd-icons

Enable the display of file names with a nerd icon prefix.

### --color-scheme

See ISSUES.md. Maybe have built-in support for a few.

### IOTACTL_FLAGS

Would also be great if we could specify default flags in the environment.

## Search

Immediate interactive highlight with `/` (highlight as you type implementation).

Ensure searching does not block the main thread, and that we terminate searches if their results will no longer be used.

Maybe even indicate with a little pulsating yellow circle in the bottom right that searching is happening.

The filtering should probably be scoped to a particular column, and we can change filtered columns to a yellow hue (dark yellow when unselected, brighter yellow when selected).

### Filtering

This depends on a great search feature having already been implemented! But see if we can have a toggle that hides all lines that aren't shown by the current filter. This toggle probably needs to be column-specific.

## Open files directly

As in, be able to open not just directories, but allow for the case where one previews a file directly.

