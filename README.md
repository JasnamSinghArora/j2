# J2

A fast, simple programming language with **automatic parallelism** — write ordinary loops and function calls; J2's compiler finds the parallelism and uses your cores for you.

- **Instant start** — `j file.j2` runs immediately (interpreter-first, no build step).
- **Native speed on demand** — `j2 build file.j2 -o out` compiles to a native binary and **auto-parallelizes** it: no annotations, no threads, no locks in your code.
- **Self-contained** — one download. No package manager, no toolchain to install, no network needed.
- **Tooling included** — a VS Code extension with syntax highlighting, one-key run, and inline errors.

```
# montecarlo.j2 — the four count_hits calls are independent + pure,
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

Download the latest `j-<version>-aarch64-apple-darwin.tar.gz` from
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

Native builds (`j2 build`) link with the system linker — if you don't have the Xcode Command Line Tools yet, run `xcode-select --install` once.

## Documentation

**[jasnamsinghArora.github.io/j](https://jasnamsingharora.github.io/j/)** — getting started, the language guide, the standard library (18 modules), the CLI, and how the automatic parallelism works.

## VS Code extension

Download `j2-lang-<version>.vsix` from [Releases](https://github.com/JasnamSinghArora/j2/releases) and run:

```sh
code --install-extension j2-lang-0.1.0.vsix
```

You get syntax highlighting for `.j2` files, a ▶ Run button (`Cmd+Shift+R`), and inline error squiggles on save.

## CLI at a glance

| Command | What it does |
|---|---|
| `j file.j2` | Run instantly (interpreter) |
| `j2 build file.j2 -o out` | Compile to a native, auto-parallelized binary |
| `j2 --version` / `j --help` | The usual |

## License

Dual-licensed under either the [MIT license](LICENSE-MIT) or the
[Apache License 2.0](LICENSE-APACHE), at your option. The release bundle
also redistributes open-source components — see
[THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
