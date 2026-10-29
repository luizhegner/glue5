#!/usr/bin/env bash
# The game's code reaches the Build workflow through a draft release of the
# repository, and the apps built from it go back into the same draft: a
# draft is seen only by those who can write to the repository (and by
# workflows with `contents: write`). Published, it would give the game to
# everyone, so a release with it is put back to draft.
#
#   draft.sh guard [RELEASE]     every published release with the game's code (or only RELEASE) back to draft
#   draft.sh fetch DIR           the newest draft's game.wasm (or .zip, .gz), checked, as DIR/b/<build>/game.wasm
#   draft.sh get RELEASE NAME    a file of the draft, if it has it, into the current directory
#   draft.sh upload RELEASE FILE...  files into the draft, replacing those of the same name
#
# As in a workflow: GH_TOKEN, GITHUB_REPOSITORY; fetch writes the draft's id
# to GITHUB_OUTPUT (release=).
set -euo pipefail

repo=${GITHUB_REPOSITORY:?}
root=$(cd "$(dirname "$0")/../.." && pwd)

# One annotation, however many lines.
die() {
    local msg="$*"
    msg=${msg//'%'/%25}
    echo "::error::${msg//$'\n'/%0A}" >&2
    exit 1
}

# The files that must stay private: the game's code (game.wasm, also zipped
# or gzipped: any name with "wasm" in it) and what is built from it.
private='(.name | ascii_downcase | contains("wasm")) or (.name | startswith("Glue5-"))'

# The game in a draft: game.wasm, or game.wasm zipped or gzipped, uploaded
# to the end.
game='.state == "uploaded" and (.name | ascii_downcase | contains("wasm") and test("\\.(wasm|zip|gz)$"))'

releases() {
    gh api --paginate --slurp "repos/$repo/releases" | jq '[.[][]]'
}

sha256() { { sha256sum "$1" 2>/dev/null || shasum -a 256 "$1"; } | cut -d' ' -f1; }

# The size of each supported build (BUILDS in crates/glue5-game/build/main.rs
# lists their sha256), only to say why another file is not one.
sizes='8b0b5899ed 63201802'

# 63201802 as 63,201,802 bytes.
bytes() {
    local n=$1 s=
    while [ ${#n} -gt 3 ]; do
        s=,${n: -3}$s
        n=${n:0:${#n}-3}
    done
    echo "$n$s bytes"
}

# Why game.wasm (FILE, its sha256 SUM) is not a supported build.
unsupported() {
    local size want supported
    size=$(wc -c <"$1" | tr -d ' ')
    supported=$(while read -r _ want; do bytes "$want"; done <<<"$sizes" | awk 'NR > 1 { printf " or " } { printf "%s", $0 }')
    while read -r _ want; do
        [ "$size" != "$want" ] ||
            die "game.wasm ($(bytes "$size"), sha256 $2) has the size of the supported game.wasm but not its contents: a changed or damaged copy. Upload the game's own game.wasm."
    done <<<"$sizes"
    die "game.wasm is $(bytes "$size") (sha256 $2); the supported one is $supported: an incomplete copy, the wrong file or another version of the game. Upload the game.wasm of the supported version."
}

guard() {
    local ids id release name failed=0
    if [ -n "${1:-}" ]; then
        ids=$1
    else
        ids=$(releases | jq -r ".[] | select(.draft | not) | select(any(.assets[]; $private)) | .id")
    fi
    for id in $ids; do
        # Its state now, not the event's.
        release=$(gh api "repos/$repo/releases/$id")
        [ "$(jq -r .draft <<<"$release")" = false ] || continue
        jq -e "any(.assets[]; $private)" <<<"$release" >/dev/null || continue
        name=$(jq -r 'if (.name // "") != "" then .name else .tag_name end' <<<"$release")
        failed=1
        if gh api -X PATCH "repos/$repo/releases/$id" -F draft=true >/dev/null; then
            echo "::error::Release \"$name\" was published with the game's code or an app built from it: it is a draft again. Keep it a draft: published, anyone can download its files."
            continue
        fi
        # An immutable release cannot go back to draft: its files go, or the
        # release itself.
        echo "::error::Release \"$name\" could not go back to draft: deleting its private files"
        jq -r ".assets[] | select($private) | .id" <<<"$release" | while read -r asset; do
            gh api -X DELETE "repos/$repo/releases/assets/$asset" ||
                { gh api -X DELETE "repos/$repo/releases/$id" && break; }
        done
        # Without the rights for either (a token limited by the account's
        # settings), only its owner can.
        if gh api "repos/$repo/releases/$id" 2>/dev/null | jq -e "(.draft | not) and any(.assets[]; $private)" >/dev/null; then
            echo "::error::Release \"$name\" is still published with the game's code: delete it now (Releases, the release, Delete)."
        fi
    done
    return $failed
}

# The game's code in the repository's files, which anyone can download.
committed() {
    local files
    files=$(git -C "$root" ls-files | grep -i '\.wasm\(\.zip\|\.gz\)\?$' || true)
    [ -n "$files" ] || return 0
    die "game.wasm is in this repository's files ($(tr '\n' ' ' <<<"$files" | sed 's/ $//')): anyone can download it from there. Delete it (open the file, ⋯, Delete file) and put it in a draft release instead (docs/github-build.md). It also stays in the history: to remove it for good, delete the repository (Settings, Delete this repository), fork again and make the draft again."
}

# What the releases have, when none is a draft with the game.
missing() {
    local all=$1 seen
    seen=$(jq -r '.[] | "- " + (if .draft then "draft" else "published release" end)
        + " \"" + (if (.name // "") != "" then .name else .tag_name end) + "\": "
        + (if .assets == [] then "no files" else
            [.assets[] | .name + (if .state != "uploaded" then " (upload not finished)" else "" end)] | join(", ") end)' <<<"$all")
    printf 'No draft release with game.wasm. Releases > Draft a new release (%s/%s/releases/new): attach game.wasm, wait until it has uploaded, and press "Save draft" (not "Publish release").' \
        "$GITHUB_SERVER_URL" "$repo"
    if [ -z "$seen" ]; then
        printf '\nThis repository has no release.'
        return
    fi
    printf '\nIts releases:\n%s' "$seen"
    if jq -e 'any(.[]; any(.assets[]; .state != "uploaded"))' <<<"$all" >/dev/null; then
        printf '\nA file whose upload did not finish is not read: delete it from the draft (Edit) and attach it again.'
    fi
    if jq -e 'any(.[]; any(.assets[]; .name | ascii_downcase | test("\\.(zip|gz|7z|rar)$") and (test("wasm.*\\.(zip|gz)$") | not)))' <<<"$all" >/dev/null; then
        printf '\nAn archive is read only as game.wasm.zip or game.wasm.gz, with game.wasm in it.'
    fi
    if jq -e 'any(.[]; .draft | not)' <<<"$all" >/dev/null; then
        printf '\nA published release is not read: only a draft is.'
    fi
}

fetch() {
    local dir=$1 all release id asset name sum build files
    guard || die "a published release had the game's code; see above"
    committed
    all=$(releases)
    release=$(jq "[.[] | select(.draft) | select(any(.assets[]; $game))] | sort_by(.created_at) | last // empty" <<<"$all")
    [ -n "$release" ] || die "$(missing "$all")"
    id=$(jq -r .id <<<"$release")
    asset=$(jq -c "[.assets[] | select($game)] | if length == 1 then .[0] else empty end" <<<"$release")
    [ -n "$asset" ] || die "The draft \"$(jq -r .name <<<"$release")\" has several game files ($(jq -r "[.assets[] | select($game) | .name] | join(\", \")" <<<"$release")): keep one."
    name=$(jq -r .name <<<"$asset")

    rm -rf "$dir/download" && mkdir -p "$dir/download"
    curl -fsSL -H "Authorization: Bearer $GH_TOKEN" -H "Accept: application/octet-stream" \
        -o "$dir/download/file" "$GITHUB_API_URL/repos/$repo/releases/assets/$(jq -r .id <<<"$asset")"
    # game.wasm as it is, or out of the archive: from a zip, its only .wasm
    # (a Mac's zip adds copies under __MACOSX/), with unzip or 7-Zip
    # (Windows).
    case $(tr '[:upper:]' '[:lower:]' <<<"$name") in
    *.wasm) mv "$dir/download/file" "$dir/game.wasm" ;;
    *.gz) gzip -dc "$dir/download/file" >"$dir/game.wasm" || die "$name is not a gzip file: upload game.wasm itself." ;;
    *.zip)
        mkdir "$dir/download/zip"
        if command -v unzip >/dev/null; then
            unzip -qq -C "$dir/download/file" '*.wasm' -d "$dir/download/zip" || true
        else
            7z x -y -bso0 -bsp0 -ssc- -o"$dir/download/zip" "$dir/download/file" '*.wasm' -r || true
        fi
        files=$(find "$dir/download/zip" -type f -iname '*.wasm' ! -path '*/__MACOSX/*')
        [ "$(grep -c . <<<"$files")" = 1 ] || die "$name should hold one game.wasm, it holds $(grep -c . <<<"$files") .wasm files: zip game.wasm alone, or upload game.wasm itself."
        mv "$files" "$dir/game.wasm"
        ;;
    esac
    rm -rf "$dir/download"
    sum=$(sha256 "$dir/game.wasm")
    # The supported builds, as the game's build script lists them.
    build=$(grep -o '("[0-9a-z]*", "[0-9a-f]\{64\}")' "$root/crates/glue5-game/build/main.rs" |
        tr -d '(",)' | awk -v sum="$sum" '$2 == sum { print $1 }')
    [ -n "$build" ] || unsupported "$dir/game.wasm" "$sum"
    mkdir -p "$dir/b/$build"
    mv "$dir/game.wasm" "$dir/b/$build/game.wasm"
    echo "game.wasm: build $build, from $name in the draft \"$(jq -r .name <<<"$release")\""
    echo "release=$id" >>"${GITHUB_OUTPUT:-/dev/null}"
}

# The draft RELEASE, or fail if it is not one any more.
draft() {
    local release
    release=$(gh api "repos/$repo/releases/$1")
    if [ "$(jq -r .draft <<<"$release")" != true ]; then
        guard "$1" || true
        die "The release is not a draft any more: nothing goes into it."
    fi
    printf '%s' "$release"
}

get() {
    local asset
    asset=$(draft "$1" | jq -r --arg name "$2" '.assets[] | select(.name == $name) | .id')
    [ -n "$asset" ] || return 0
    curl -fsSL -H "Authorization: Bearer $GH_TOKEN" -H "Accept: application/octet-stream" \
        -o "$2" "$GITHUB_API_URL/repos/$repo/releases/assets/$asset"
}

upload() {
    local id=$1 release url file name old
    shift
    for file; do
        release=$(draft "$id")
        name=${file##*/}
        old=$(jq -r --arg name "$name" '.assets[] | select(.name == $name) | .id' <<<"$release")
        [ -z "$old" ] || gh api -X DELETE "repos/$repo/releases/assets/$old"
        url=$(jq -r '.upload_url | sub("\\{.*"; "")' <<<"$release")
        curl -fsS -X POST -H "Authorization: Bearer $GH_TOKEN" -H "Content-Type: application/octet-stream" \
            --data-binary @"$file" "$url?name=$name&label=$name%20(${commit:0:7})" >/dev/null
        echo "uploaded $name to the draft \"$(jq -r .name <<<"$release")\""
    done
}

: "${GITHUB_API_URL:=https://api.github.com}" "${GITHUB_SERVER_URL:=https://github.com}"
commit=${GITHUB_SHA:-$(git -C "$root" rev-parse HEAD)}
command=${1:-}
shift || true
case $command in
guard | fetch | get | upload) "$command" "$@" ;;
*) die "usage: draft.sh guard [RELEASE] | fetch DIR | get RELEASE NAME | upload RELEASE FILE..." ;;
esac
