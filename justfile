# Build an optimized binary for the current platform.
build:
    devenv shell -- ram-build

# Build and package the release artifacts for the current builder platform.
dist:
    #!/usr/bin/env bash
    set -euo pipefail

    case "$(uname -s)" in
      Darwin)
        task=release:build-apple
        ;;
      Linux)
        task=release:build-linux-windows
        ;;
      *)
        echo "Unsupported release builder platform: $(uname -s)" >&2
        exit 1
        ;;
    esac

    devenv tasks run "$task"
