# J2

J2 is a programming language with an interpreter and a native compiler. Native
builds can run independent loops and function calls across multiple CPU cores.

- Run a program with `j file.j2` without a build step.
- Compile a native binary with `j2 build file.j2 -o out`. The compiler
  parallelizes supported patterns when it can establish that they are safe.
- The download includes the compiler, runtime, and libraries for offline builds.
- The VS Code extension provides syntax highlighting, a Run command, and inline errors.

```
# montecarlo.j2: the four count_hits calls are independent and pure,
# so `j2 build` runs them on separate cores automatically.
func estimate(n: int) -> float = {
    a := count_hits(1, n)
    b := count_hits(999983, n)
    c := count_hits(50000017, n)
    e := count_hits(1234567891, n)
    total := a + b + c + e
    give (total * 4.0) / (4.0 * (n * 1.0))
}
```

## Install (macOS, Apple Silicon)

Download the latest `j2-<version>-aarch64-apple-darwin.tar.gz` from
[**Releases**](https://github.com/JasnamSinghArora/j2/releases), then:

```sh
tar xzf j2-0.1.0-aarch64-apple-darwin.tar.gz
cd j2-0.1.0-aarch64-apple-darwin
./install.sh
```

This installs J2 under `~/.j2` and puts the `j2` command on your PATH. The binaries are signed and notarized by Apple. Try it:

```sh
echo 'print("hello, world")' > hello.j2
j hello.j2
```

Native builds (`j2 build`) use the system linker. If the Xcode Command Line Tools
are missing, install them with `xcode-select --install`.

## Documentation

[j2-lang.org](https://j2-lang.org) covers installation, the language, the standard
library, CLI commands, and automatic parallelism.

## VS Code extension

Download `j2-lang-<version>.vsix` from [Releases](https://github.com/JasnamSinghArora/j2/releases) and run:

```sh
code --install-extension j2-lang-0.1.0.vsix
```

The extension highlights `.j2` files, adds a Run button (`Cmd+Shift+R`), and shows
inline errors on save.

## Commands

| Command | What it does |
|---|---|
| `j file.j2` | Run instantly (interpreter) |
| `j2 build file.j2 -o out` | Compile to a native, auto-parallelized binary |
| `j2 --version` / `j --help` | Show version or help |

## Source

The compiler source is in [`source/`](source/): the frontend under
`compiler/j2_compiler`, the auto-parallelization passes under
`compiler/j2_passes`, the runtime under `library/j2_runtime`, the driver under
`src/bin/j2`, and the tests under `j2-tests`. See
[`source/README.md`](source/README.md) for the full layout.

## License

J2 uses the [MIT license](LICENSE-MIT). Bundled third-party components keep
their own licenses; see [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
