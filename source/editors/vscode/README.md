# J2 for Visual Studio Code

Syntax highlighting, a Run command, and inline parse/type errors for J2 v2.

## Features

- Syntax highlighting for `.j2` files, including keywords, strings, numbers,
  comments, function/class definitions, types, and operators.
- Run the current file with `Cmd/Ctrl+Shift+R`, the Run button in the editor,
  or `J2: Run File` in the command palette. The output appears in an integrated
  terminal.
- Show parse/type errors at their line and column when you open or save a file.

## Requirements

Install [J2 v2](https://j2-lang.org/download.html). The `j2` executable must be
on your `PATH`, or you can set `j2.path` to its full path.

## Install

Download `j2-lang-0.2.0.vsix` from the
[v2 release](https://github.com/JasnamSinghArora/j2/releases/tag/v0.2.0), then run:

```sh
code --install-extension j2-lang-0.2.0.vsix
```

To package the extension from this directory:

```sh
npx @vscode/vsce package
```

## Settings

- `j2.path`: path to the `j2` executable, defaulting to `j2`.
- `j2.checkOnSave`: show inline errors on open/save, enabled by default.

For native builds with automatic parallelism, run
`j2 build file.j2 -o out` in a terminal.
