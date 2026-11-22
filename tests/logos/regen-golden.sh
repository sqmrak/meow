#!/usr/bin/env bash
set -euo pipefail

# rebuilds tests/golden from the perl logos
# LOGOS_ORACLE: path to logos.pl, use the revision before theos/logos#114,
# newer revisions cut the rest of the line after a %orig without arguments
#
# one file per source: `@@ <generator> <warnings> <exit status>` opens a run,
# `! ` lines are stderr and `> ` lines are stdout

here="$(cd "$(dirname "$0")" && pwd)"
: "${LOGOS_ORACLE:?set LOGOS_ORACLE to the path of logos.pl}"
[ -f "$LOGOS_ORACLE" ] || { echo "no such file: $LOGOS_ORACLE" >&2; exit 1; }

mkdir -p "$here/golden"
cd "$here/corpus"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

for src in *.x; do
	name="${src%.x}"
	runs="MobileSubstrate:default libhooker:default internal:default"
	case "$name" in
		warn_*) runs="$runs MobileSubstrate:error MobileSubstrate:none" ;;
	esac
	: > "$here/golden/$name.golden"
	for run in $runs; do
		gen="${run%%:*}"
		warnings="${run##*:}"
		status=0
		perl "$LOGOS_ORACLE" -c "generator=$gen" -c "warnings=$warnings" "$src" > "$tmp/out" 2> "$tmp/err" || status=$?
		{
			echo "@@ $gen $warnings $status"
			sed 's/^/! /' "$tmp/err"
			sed 's/^/> /' "$tmp/out"
		} >> "$here/golden/$name.golden"
	done
done

# logify: same layout, the run line holds the options
logify="$(dirname "$LOGOS_ORACLE")/logify.pl"
: > "$here/logify/sample.golden"
cd "$here/logify"
for opts in "" "-i reset,contains:,initWithTitle:count:" "-e reset,center"; do
	status=0
	# shellcheck disable=SC2086
	perl "$logify" $opts sample.h > "$tmp/out" 2> "$tmp/err" || status=$?
	{
		echo "@@ $opts|$status"
		sed 's/^/! /' "$tmp/err"
		sed 's/^/> /' "$tmp/out"
	} >> "$here/logify/sample.golden"
done
