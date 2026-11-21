# Homebrew formula for J2 on macOS
class J2 < Formula
  desc "The J2 programming language with automatic parallelism"
  homepage "https://j2-lang.org"
  version "0.2.0"
  url "https://github.com/JasnamSinghArora/j2/releases/download/v#{version}/j2-#{version}-aarch64-apple-darwin.tar.gz"
  sha256 "0fb52d7afab0aa0330d110ac7ffa917942fa5cf04fc9fd031552cbede6f8fdd3"
  license "MIT"

  def install
    # Keep the compiler, dependencies, and runtime beside the driver.
    libexec.install Dir["*"]
    bin.install_symlink libexec/"bin/j2"
  end

  def caveats
    "Run a program with `j2 file.j2`, or compile it with `j2 build file.j2 -o out`."
  end

  test do
    assert_match "j2 #{version}", shell_output("#{bin}/j2 --version")
    (testpath/"hello.j2").write 'print(map([1, 2, 3,], _ * 2,))'
    assert_match "[2, 4, 6]", shell_output("#{bin}/j2 #{testpath}/hello.j2")
  end
end
