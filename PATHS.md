```
zip://[fs://foo/bar.zip]/xyz
```

If you had a file with a `[` or `]` in it, you could use the doubling-up rule:

```
zip://[fs://foo/[[quote]].zip]/xyz
```

Nest further:

```
zip://[zip://[fs://foo/[[quote]].zip]/xyz.zip]/foo.md
```

This requires that `]]` will never appear through nesting alone (this is trivially the case with `[[`).

Thus:

```
zip://[zip://[fs://foo/quote]]
```

Must be illegal, which you might do by requiring trailing slashes:

```
zip://[zip://[fs://foo/quote]/]/
```

In the protocol definition for `zip`. (Or perhaps more likely `zip+in`).
