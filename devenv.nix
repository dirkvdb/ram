{ ... }:

{
  languages.rust = {
    enable = true;
    channel = "nixpkgs";
  };

  scripts = {
    ram-build = {
      description = "Build an optimized ram binary";
      exec = "cargo build --release";
    };
    ram-check = {
      description = "Run formatting, lint, tests, and a release build";
      exec = ''
        cargo fmt --all -- --check
        cargo clippy --all-targets --all-features -- -D warnings
        cargo test --all-targets --all-features
        cargo build --release
      '';
    };
    ram-run = {
      description = "Run the optimized ram binary (passes through arguments)";
      exec = "cargo run --release -- \"$@\"";
    };
  };

  enterTest = ''
    ram-check
  '';
}
