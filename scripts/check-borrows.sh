#!/bin/bash
# Refuse a borrow guard that outlives the statement that created it.
#
# `if let Some(x) = cell.borrow_mut().take() { ... }` keeps the guard alive for
# the whole block, because an `if let` scrutinee is not a terminating scope the
# way an `if` condition is. Inside a GTK callback that re-enters the same
# RefCell, the process aborts. This has caused two separate crashes in this
# codebase, both of them found by a user rather than by a test, so the shape is
# banned outright: read into a local first, or go through a method that returns
# owned data.
set -u
cd "$(dirname "$0")/.."
# `for x in cell.borrow().iter()` holds the guard for the whole loop too, for
# the same reason: the temporary lives to the end of the statement, and a for
# loop is one statement.
HITS=$(grep -rnE '^\s*(} )?(if|while) let .*\.borrow(_mut)?\(\)|^\s*match .*\.borrow(_mut)?\(\)|^\s*for .* in .*\.borrow(_mut)?\(\)' src/ || true)
if [ -n "$HITS" ]; then
  echo "borrow guard held across a block:"
  echo "$HITS"
  echo
  echo "Read the value into a local first, or add a method that returns owned data."
  exit 1
fi
echo "  no borrow guards held across a block"
