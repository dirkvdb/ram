{ lib, pkgs, ... }:

let
  isDarwin = pkgs.stdenv.hostPlatform.isDarwin;
  isLinux = pkgs.stdenv.hostPlatform.isLinux;
  releaseTag = "v${(fromTOML (builtins.readFile ./Cargo.toml)).package.version}";
  llvmMingw =
    if isLinux then
      (pkgs.callPackage "${pkgs.path}/pkgs/applications/emulators/wine/llvm-mingw.nix" { }).overrideAttrs
        (old: {
          buildInputs = (old.buildInputs or [ ]) ++ [ pkgs.zstd ];
          # The prebuilt x86_64 archive's LLDB needs libraries that cross-linking does not use.
          postInstall = (old.postInstall or "") + ''
            rm -f "$out"/bin/lldb* "$out"/lib/liblldb*
          '';
        })
    else
      null;

in
{
  languages.rust = {
    enable = true;
    channel = "stable";
    targets =
      lib.optionals isDarwin [ "aarch64-apple-darwin" ]
      ++ lib.optionals isLinux [
        "aarch64-pc-windows-gnullvm"
        "aarch64-unknown-linux-musl"
        "x86_64-pc-windows-gnu"
        "x86_64-unknown-linux-musl"
      ];
  };

  packages = [
    pkgs.gnutar
    pkgs.jq
    pkgs.just
  ]
  ++ lib.optionals isLinux [
    pkgs.cargo-zigbuild
    pkgs.file
    pkgs.gh
    pkgs.pkgsCross.mingwW64.stdenv.cc
    pkgs.zip
    pkgs.zig
  ];

  env = lib.optionalAttrs isLinux {
    CARGO_TARGET_AARCH64_PC_WINDOWS_GNULLVM_LINKER = "${llvmMingw}/bin/aarch64-w64-mingw32-clang";
    CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = "${pkgs.pkgsCross.mingwW64.stdenv.cc}/bin/x86_64-w64-mingw32-gcc";
    CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS = "-L native=${pkgs.pkgsCross.mingwW64.windows.pthreads}/lib";
  };

  scripts = {
    ram-build = {
      description = "Build an optimized ram binary";
      exec = "cargo build --release --locked";
    };
    ram-build-linux-static = {
      description = "Build static x86_64 and ARM64 Linux binaries";
      exec = ''
        set -euo pipefail
        cargo zigbuild --release --locked --target x86_64-unknown-linux-musl
        cargo zigbuild --release --locked --target aarch64-unknown-linux-musl
      '';
    };
    ram-build-windows = {
      description = "Build optimized x86_64 and ARM64 Windows binaries";
      exec = ''
        set -euo pipefail
        cargo build --release --locked --target x86_64-pc-windows-gnu
        cargo build --release --locked --target aarch64-pc-windows-gnullvm
      '';
    };
    ram-check = {
      description = "Run formatting, lint, tests, and a native release build";
      exec = ''
        set -euo pipefail
        cargo fmt --all -- --check
        cargo clippy --all-targets --all-features -- -D warnings
        cargo test --all-targets --all-features
        cargo build --release --locked
      '';
    };
    ram-run = {
      description = "Run the optimized ram binary (passes through arguments)";
      exec = "cargo run --release -- \"$@\"";
    };
  };

  tasks =
    lib.optionalAttrs isDarwin {
      "release:build-apple" = {
        description = "Build and package the Apple ARM64 release";
        input.tag = releaseTag;
        exec = ''
          set -euo pipefail
          RELEASE_TAG=$(jq -r .tag <<< "$DEVENV_TASK_INPUT")
          cargo build --release --locked --target aarch64-apple-darwin
          mkdir -p dist
          tar -C target/aarch64-apple-darwin/release -czf \
            "dist/ram-$RELEASE_TAG-macos-aarch64.tar.gz" ram
        '';
      };
    }
    // lib.optionalAttrs isLinux {
      "release:build-linux-windows" = {
        description = "Build, verify, and package the Linux and Windows releases";
        input.tag = releaseTag;
        exec = ''
          set -euo pipefail
          RELEASE_TAG=$(jq -r .tag <<< "$DEVENV_TASK_INPUT")
          ram-build-windows
          ram-build-linux-static

          file target/x86_64-pc-windows-gnu/release/ram.exe \
            | grep -q "PE32+ executable.*x86-64"
          file target/aarch64-pc-windows-gnullvm/release/ram.exe \
            | grep -q "PE32+ executable.*ARM64"
          file target/x86_64-unknown-linux-musl/release/ram \
            | grep -q "ELF 64-bit.*x86-64.*statically linked"
          file target/aarch64-unknown-linux-musl/release/ram \
            | grep -q "ELF 64-bit.*ARM aarch64.*statically linked"

          for binary in \
            target/x86_64-unknown-linux-musl/release/ram \
            target/aarch64-unknown-linux-musl/release/ram
          do
            if ${pkgs.binutils}/bin/readelf -l "$binary" | grep -q INTERP; then
              echo "$binary has a dynamic interpreter" >&2
              exit 1
            fi
          done

          mkdir -p dist
          zip -j "dist/ram-$RELEASE_TAG-windows-x86_64.zip" \
            target/x86_64-pc-windows-gnu/release/ram.exe
          zip -j "dist/ram-$RELEASE_TAG-windows-aarch64.zip" \
            target/aarch64-pc-windows-gnullvm/release/ram.exe
          tar -C target/x86_64-unknown-linux-musl/release -czf \
            "dist/ram-$RELEASE_TAG-linux-x86_64-musl.tar.gz" ram
          tar -C target/aarch64-unknown-linux-musl/release -czf \
            "dist/ram-$RELEASE_TAG-linux-aarch64-musl.tar.gz" ram
        '';
      };

      "release:publish" = {
        description = "Publish a GitHub release";
        input.tag = releaseTag;
        exec = ''
          set -euo pipefail
          RELEASE_TAG=$(jq -r .tag <<< "$DEVENV_TASK_INPUT")
          rm -f dist/SHA256SUMS
          (cd dist && sha256sum ram-* > SHA256SUMS)

          if gh release view "$RELEASE_TAG" >/dev/null 2>&1; then
            gh release upload "$RELEASE_TAG" dist/* --clobber
          else
            gh release create "$RELEASE_TAG" dist/* \
              --verify-tag \
              --title "$RELEASE_TAG" \
              --generate-notes
          fi
        '';
      };
    };

  enterTest = ''
    ram-check
  '';
}
