# Homebrew formula for J (macOS). Published from the release at
# https://github.com/JasnamSinghArora/j/releases — to offer `brew install`,
# put this file in a public repo named `homebrew-j` as Formula/j.rb, then:
#   brew tap JasnamSinghArora/j && brew install j
class J < Formula
  desc "The J programming language (native + auto-parallel, interpreter-first)"
  homepage "https://github.com/JasnamSinghArora/j"
  version "0.1.1"
  url "https://github.com/JasnamSinghArora/j/releases/download/v#{version}/j-#{version}-aarch64-apple-darwin.tar.gz"
  sha256 "029e31a0806c9072cc5b9ba6974734ff19f00be542606a5a59ce90917d0bf973"
  license any_of: ["MIT", "Apache-2.0"]

  def install
    # The bundle is self-contained (toolchain + vendored deps + runtime); the
    # `j` binary discovers them relative to itself, so install it wholesale.
    libexec.install Dir["*"]
    bin.install_symlink libexec/"bin/j"
  end

  def caveats
    "J runs interpreter-first (instant). For native + auto-parallel speed: `j build file.j -o out`."
  end

  test do
    (testpath/"h.j").write 'print("ok")'
    assert_match "ok", shell_output("#{bin}/j #{testpath}/h.j")
  end
end
