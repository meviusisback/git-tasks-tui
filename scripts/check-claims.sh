#!/bin/sh
# Refuse a commit whose message **claims** something the staged tree does not show.
#
# The failure this exists for happened three times in one session: a commit
# declared "the merge is in", "the fix is applied", "the symbol is there" — and the
# tree said otherwise. The message describes the intent, the tree describes the
# content, and only the tree is what runs.
#
# Prose cannot be checked mechanically, so this checks a **form**. Any line in the
# message shaped
#
#     Claims: <path> contains "<text>"
#     Claims: <path> absent   "<text>"
#
# is verified against the **staged** content (`git show :<path>`), i.e. exactly
# what the commit will contain. Nothing else in the message is inspected: there is
# no NLP here, and there should not be.
#
# Exit 0 = every claim holds (or there is none). Exit 1 = at least one is false.
# Install as `.git/hooks/commit-msg`; the hook receives the message file.

set -eu

msg_file=${1:-}
if [ -z "$msg_file" ] || [ ! -f "$msg_file" ]; then
    echo "check-claims: no message file given" >&2
    exit 1
fi

claims=$(grep -E '^Claims: ' "$msg_file" 2>/dev/null || true)
[ -n "$claims" ] || exit 0

failures=$(mktemp)
trap 'rm -f "$failures"' EXIT INT TERM

# Read the claims from a here-doc, not a pipe, so the loop runs in this shell.
while IFS= read -r line; do
    [ -n "$line" ] || continue
    spec=${line#Claims: }

    case "$spec" in
        *" contains "*) verb=contains; path=${spec%% contains *}; text=${spec#* contains } ;;
        *" absent "*)   verb=absent;   path=${spec%% absent *};   text=${spec#* absent } ;;
        *)
            echo "check-claims: cannot parse '$line' (expected: <path> contains|absent \"<text>\")" >&2
            echo x >> "$failures"
            continue
            ;;
    esac

    path=$(printf '%s' "$path" | sed 's/^ *//; s/ *$//')
    # Keep only what is inside the quotes: prose after the closing quote must not
    # be able to change the verdict.
    text=$(printf '%s' "$text" | sed 's/^ *//; s/^"//; s/".*$//')

    # A file that is not in the commit witnesses nothing — same rule as K-1 in
    # review-state.sh, and for the same reason.
    if ! git cat-file -e ":$path" 2>/dev/null; then
        echo "check-claims: FALSE CLAIM — '$path' is not in this commit, so it cannot witness $verb \"$text\"" >&2
        echo x >> "$failures"
        continue
    fi

    if git show ":$path" | grep -qF -- "$text"; then found=yes; else found=no; fi

    if { [ "$verb" = contains ] && [ "$found" = yes ]; } \
        || { [ "$verb" = absent ] && [ "$found" = no ]; }; then
        :
    else
        # Name the right object. Saying "'$path' is absent" made a reviewer check
        # `git add` for a file that was right there: the verdict is about the
        # TEXT inside the file, never about the file.
        if [ "$verb" = contains ]; then
            echo "check-claims: FALSE CLAIM — '$path' is in this commit but does not contain: \"$text\"" >&2
        else
            echo "check-claims: FALSE CLAIM — '$path' does contain: \"$text\", and the message claims it does not" >&2
        fi
        echo x >> "$failures"
    fi
done <<EOF
$claims
EOF

if [ -s "$failures" ]; then
    echo "check-claims: the commit was refused; fix the message or the tree, not the check." >&2
    exit 1
fi
exit 0
