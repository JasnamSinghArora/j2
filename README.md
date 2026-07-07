# J

A fast, simple programming language with **automatic parallelism** — write ordinary loops and function calls; J's compiler finds the parallelism and uses your cores for you.

- **Instant start** — `j file.j` runs immediately (interpreter-first, no build step).
- **Native speed on demand** — `j build file.j -o out` compiles to a native binary and **auto-parallelizes** it: no annotations, no threads, no locks in your code.
- **Self-contained** — one download. No package manager, no toolchain to install, no network needed.
- **Tooling included** — a VS Code extension with syntax highlighting, one-key run, and inline errors.

```
# montecarlo.j — the four count_hits calls are independent + pure,
# so `j build` runs them on separate cores automatically.
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

Download the latest `j-<version>-aarch64-apple-darwin.tar.gz` from
[**Releases**](https://github.com/JasnamSinghArora/j/releases), then:

```sh
tar xzf j-0.1.0-aarch64-apple-darwin.tar.gz
cd j-0.1.0-aarch64-apple-darwin
./install.sh
```

This installs J under `~/.j` and puts the `j` command on your PATH. The binaries are signed and notarized by Apple. Try it:

```sh
echo 'print("hello, world")' > hello.j
j hello.j
```

Native builds (`j build`) link with the system linker — if you don't have the Xcode Command Line Tools yet, run `xcode-select --install` once.

## Documentation

**[jasnamsinghArora.github.io/j](https://jasnamsingharora.github.io/j/)** — getting started, the language guide, the standard library (18 modules), the CLI, and how the automatic parallelism works.

## VS Code extension

Download `j-lang-<version>.vsix` from [Releases](https://github.com/JasnamSinghArora/j/releases) and run:

```sh
code --install-extension j-lang-0.1.0.vsix
```

You get syntax highlighting for `.j` files, a ▶ Run button (`Cmd+Shift+R`), and inline error squiggles on save.

## CLI at a glance

| Command | What it does |
|---|---|
| `j file.j` | Run instantly (interpreter) |
| `j build file.j -o out` | Compile to a native, auto-parallelized binary |
| `j --version` / `j --help` | The usual |

## License

Dual-licensed under either the [MIT license](LICENSE-MIT) or the
[Apache License 2.0](LICENSE-APACHE), at your option. The release bundle
also redistributes open-source components — see
[THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
