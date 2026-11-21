# J2

J2 is a programming language with an interpreter and a native compiler. Native
builds can run independent loops and function calls across multiple CPU cores.

## Download v2

J2 v2 (`0.2.0`) is the current release for macOS on Apple Silicon. Get the
[installer or archive](https://j2-lang.org/download.html) to start using it.
Cloning this repository or downloading its `main` branch gives you the v2 source.

The previous [v1 (`0.1.0`) release](https://github.com/JasnamSinghArora/j2/releases/tag/v0.1.0)
remains available in the release history. See the
[version guide](https://j2-lang.org/docs/versions.html) and
[v2 release notes](source/V2_NOTES.md) for the changes.

- Run a program with `j2 file.j2` without a build step.
- Compile a native binary with `j2 build file.j2 -o out`. The compiler
  parallelizes supported patterns when it can establish that they are safe.
- The download includes the compiler, runtime, and libraries for offline builds.
- The VS Code extension provides syntax highlighting, a Run command, and inline errors.

## What v2 adds

More numeric maps, filters, pipes, and independent indexed writes can run in
parallel. Supported callbacks can read captured numeric arrays and use local
bindings, branches, and eligible recursive scalar functions. Small inputs stay
serial, and output order is preserved.

The `_` callback shortcut and trailing commas reduce syntax:

```j2
values = collect(1..40000)
scaled = map(values, _ * 3)
evens = filter(scaled, _ % 2 == 0)
items = [1, 2, 3,]
```

Use a native build to try the parallel paths. `J2_PARALLEL=0` runs them serially.
The [release notes](source/V2_NOTES.md) describe supported cases, fallback
behavior, correctness fixes, and validation.

## Install v2 (macOS, Apple Silicon)

Download `j2-0.2.0-aarch64-apple-darwin.tar.gz` from the
[v2 release](https://github.com/JasnamSinghArora/j2/releases/tag/v0.2.0), then:

```sh
tar xzf j2-0.2.0-aarch64-apple-darwin.tar.gz
cd j2-0.2.0-aarch64-apple-darwin
./install.sh
```

This installs J2 under `~/.j2` and puts the `j2` command on your PATH. The binaries are signed and notarized by Apple. Try it:

```sh
echo 'print("hello, world")' > hello.j2
j2 hello.j2
```

Native builds (`j2 build`) use the system linker. If the Xcode Command Line Tools
are missing, install them with `xcode-select --install`.

## Build v2 from source

Install Rust and Cargo, then build the driver:

```sh
git clone https://github.com/JasnamSinghArora/j2.git
cd j2/source
cargo build --release --manifest-path src/bin/j2/Cargo.toml
./src/bin/j2/target/release/j2 --version
./src/bin/j2/target/release/j2 build j2-examples/hello.j2 -o hello
./hello
```

The driver reports `0.2.0`. The new collection and indexed-loop helpers work
with stock Rust; the existing MIR parallelization passes require the forked
backend. See
[source build details](source/README.md).

## Documentation

[j2-lang.org](https://j2-lang.org) covers installation, the language, the standard
library, CLI commands, and automatic parallelism.

## VS Code extension

Download `j2-lang-0.2.0.vsix` from the
[v2 release](https://github.com/JasnamSinghArora/j2/releases/tag/v0.2.0) and run:

```sh
code --install-extension j2-lang-0.2.0.vsix
```

The extension highlights `.j2` files, adds a Run button (`Cmd+Shift+R`), and shows
inline errors on save.

## Commands

| Command | What it does |
|---|---|
| `j2 file.j2` | Run instantly (interpreter) |
| `j2 build file.j2 -o out` | Compile to a native, auto-parallelized binary |
| `j2 --version` / `j2 --help` | Show version or help |

## Source

The compiler source is in [`source/`](source/): the frontend under
`compiler/j2_compiler`, the auto-parallelization passes under
`compiler/j2_passes`, the runtime under `library/j2_runtime`, the driver under
`src/bin/j2`, and the tests under `j2-tests`. See
[`source/README.md`](source/README.md) for the full layout.

## License

J2 uses the [MIT license](LICENSE-MIT). Bundled third-party components keep
their own licenses; see [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
