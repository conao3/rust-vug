set -eu
awk '
    function count(range, fields) {
        return split(range, fields, ",") == 2 ? fields[2] + 0 : 1
    }
    /^@@ / {
        if (old || new || $2 !~ /^-[0-9]+(,[0-9]+)?$/ ||
            $3 !~ /^\+[0-9]+(,[0-9]+)?$/ || $4 != "@@") {
            invalid = 1
            exit
        }
        old = count($2)
        new = count($3)
        hunks++
        next
    }
    old || new {
        prefix = substr($0, 1, 1)
        if (prefix == " " || prefix == "-") old--
        if (prefix == " " || prefix == "+") new--
        if (old < 0 || new < 0 || (prefix !~ /^[ +-]$/ && prefix != "\\")) {
            invalid = 1
            exit
        }
    }
    END { exit (invalid || old || new || !hunks) }
' "$1" || { printf '%s: invalid or incomplete unified diff hunk\n' "$1" >&2; exit 1; }

status=0
log=$(LC_ALL=C patch -p1 -i "$1" 2>&1) || status=$?
printf '%s\n' "$log"
test "$status" -eq 0
if printf '%s\n' "$log" | grep -Ei 'hunk.*(failed|ignored)|hunks? ignored'; then
    exit 1
fi
if test -n "$(find . -name '*.rej' -print -quit)"; then
    printf '%s: patch reject file found\n' "$1" >&2
    exit 1
fi
