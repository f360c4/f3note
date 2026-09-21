#!/bin/bash
# End-to-end crash recovery check.
#
# Phase one opens a file, edits it, waits for autosave, then SIGKILLs itself.
# Phase two starts fresh and reports whether the unsaved work came back. The
# file on disk must be byte-identical throughout: f3note never writes to the
# user's file on its own.
set -u
BIN="$(dirname "$0")/../target/release/crashtest"
ROOT=$(mktemp -d /tmp/f3note-crashtest-XXXXXX)
FILE="$ROOT/subject.txt"
export XDG_STATE_HOME="$ROOT/state" XDG_CONFIG_HOME="$ROOT/config" XDG_DATA_HOME="$ROOT/data"
mkdir -p "$XDG_STATE_HOME" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME"

printf 'original line, saved on disk\n' > "$FILE"
BEFORE=$(sha256sum "$FILE" | cut -d' ' -f1)

echo "=== phase 1: edit, autosave, then SIGKILL ==="
"$BIN" write "$FILE"
echo "exit status: $? (137 = killed by SIGKILL)"

echo
echo "=== state written before the kill ==="
find "$XDG_STATE_HOME/f3note" -type f 2>/dev/null | sed "s|$XDG_STATE_HOME/f3note/||" | sort
echo "--- mirror ---"
cat "$XDG_STATE_HOME"/f3note/docs/*/mirror 2>/dev/null

echo
echo "=== phase 2: restart and recover ==="
"$BIN" read
RESULT=$?

echo
AFTER=$(sha256sum "$FILE" | cut -d' ' -f1)
if [ "$BEFORE" = "$AFTER" ]; then
  echo "PASS: the file on disk was never modified"
else
  echo "FAIL: f3note wrote to the user's file without being asked"
  RESULT=1
fi
cat "$FILE"

rm -rf "$ROOT"
exit $RESULT
