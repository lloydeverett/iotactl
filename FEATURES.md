
# Features

For features we'd like to have but we need to (1) think about where they fit in the UX and (2) want to ponder how best to introduce these architecturally so as not to overcomplicate the code.

## Toggle management

Putting all these toggles in the status bar / assigning keybindings is going to become untenable, so find a way to manage these. Maybe should just be commands and integrate with whatever command palette we end up implementing?

### Rendering mode

It'd be nice to have the option to render markdown by having a feature to conceal formatting characters. Maybe similar type of thing for JSON.

### Line number column for file previews

### Smooth scrolling

When using Ctrl+D, Ctrl+U, page up, page down

## Command-line flags

Also for toggling all of the toggles above through this mechanism, too!

### --nerd-icons

Enable the display of file names with a nerd icon prefix.

### --color-scheme

### IOTACTL_FLAGS

Would also be great if we could specify default flags in the environment.

## Open files directly

As in, be able to open not just directories, but allow for the case where one previews a file directly.

