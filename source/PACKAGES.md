# J2 modules

Multi-file J2 code needs no manifest, lockfile, or registry. Split code across
`.j2` files and pull them in with `import`.

## import

```
import "./util.j2"      # relative to the importing file
import "/abs/path.j2"   # absolute path
import "math/vec.j2"    # searched on the module path
```

An import inlines the file's source at that point. Imports resolve
transitively. Importing the same file twice is a no-op, so a module shared by
several files is included once.

## Module search path

A non-absolute import is tried in this order. First match wins.

1. The importing file's directory.
2. The importing file's `lib/` subdirectory.
3. Each colon-separated entry of `J2_PATH`.

```sh
export J2_PATH="$HOME/.j2-modules:/opt/team/j2lib"
```

## Project layout

```
myapp/
  main.j2         # j2 main.j2
  lib/
    parser.j2     # import "parser.j2"
    render.j2     # import "render.j2"
```

```
# main.j2
import "parser.j2"
import "render.j2"

program = parse(input())
render(program)
```

## Scope

This is a convention, not a package manager. There is no versioning,
dependency resolution, or download step. A package is a directory of `.j2`
files on the search path.
