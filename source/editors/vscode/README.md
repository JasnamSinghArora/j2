# J2 for Visual Studio Code

Syntax highlighting, a Run command, and inline parse and type errors for `.j2`
files. Not a language server.

## Features

- Highlighting for keywords, strings, numbers, comments, definitions, types,
  and operators.
- Run the current file with `Cmd/Ctrl+Shift+R`, the play button in the editor
  title bar, or `J2: Run File` in the command palette. Runs `j <file>` through
  the interpreter; output goes to an integrated terminal.
- Inline errors on open and save, placed at the reported line and column, via
  `j2 emit-native`.

## Requirements

`j` must be on `PATH`, or set `j.path`. Installing J2 is enough.

## Install

- Try it: open this folder in VS Code and press F5.
- Package it: `npm i -g @vscode/vsce && vsce package`, then
  `code --install-extension j2-lang-0.1.0.vsix`.
- Or copy this folder to `~/.vscode/extensions/j2-lang/`.

## Settings

- `j.path`: path to the `j` executable. Default `j`.
- `j.checkOnSave`: show inline errors on open and save. Default `true`.

The Run command uses the interpreter. For native speed on heavy kernels, build
with `j2 build file.j2 -o out`.
