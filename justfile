default:
    @just --list

test:
    cargo test

build:
    cargo build --release

# Requires cargo-zigbuild: `cargo install cargo-zigbuild`
build-glibc-2-31:
    cargo zigbuild --release --target aarch64-unknown-linux-gnu.2.31

release changeid:
    #!/usr/bin/env bash
    set -euo pipefail

    changeid="{{ changeid }}"

    mapfile -t commits < <(jj log --no-graph -r "$changeid" -T 'commit_id ++ "\n"')

    if [ "${#commits[@]}" -ne 1 ]; then
      echo "Expected change id to resolve to exactly one commit, got ${#commits[@]}" >&2
      exit 1
    fi

    commit="${commits[0]}"

    if ! jj log --no-graph -r "($changeid) & immutable()" --limit 1 -T 'commit_id' | grep -qx "$commit"; then
      echo "Refusing to release mutable commit: $commit" >&2
      exit 1
    fi

    date="$(git log -1 --format='%cd' --date=format:'%Y.%m.%d' "$commit")"
    count="$(git rev-list --count "$commit")"
    version="v$date.$count"

    if git rev-parse -q --verify "refs/tags/$version" >/dev/null; then
      echo "Tag already exists locally: $version" >&2
      exit 1
    fi

    if git ls-remote --exit-code --tags origin "refs/tags/$version" >/dev/null 2>&1; then
      echo "Tag already exists on origin: $version" >&2
      exit 1
    fi

    git tag "$version" "$commit"
    git push origin "refs/tags/$version"

    echo "Released $version at $commit"
