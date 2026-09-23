# Homebrew formula for J2 on macOS
class J2 < Formula
  desc "The J2 programming language (native + auto-parallel, interpreter-first)"
  homepage "https://github.com/JasnamSinghArora/j2"
  version "0.1.0"
  url "https://github.com/JasnamSinghArora/j2/releases/download/v#{version}/j2-#{version}-aarch64-apple-darwin.tar.gz"
  sha256 "6fda8338791730cf7937362acd03e29247719e65785458e62988e1789c842e75"
  license any_of: ["MIT", "Apache-2.0"]

  def install
    # The bundle is self-contained (toolchain + vendored deps + runtime); the
    # `j2` binary discovers them relative to itself, so install it wholesale.
    libexec.install Dir["*"]
    bin.install_symlink libexec/"bin/j2"
  end

  def caveats
    "J2 runs interpreter-first (instant). For native + auto-parallel speed: `j2 build file.j2 -o out`."
  end

  test do
    (testpath/"h.j2").write 'print("ok")'
    assert_match "ok", shell_output("#{bin}/j2 #{testpath}/h.j2")
  end
end
