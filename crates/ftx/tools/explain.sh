#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# For each signal jt9 decoded and we missed: can our demodulator decode it
# at jt9's frequency and time, in the original audio ("orig") or in what
# is left after our own subtraction ("resid")? Prints a summary per class.
#   explain.sh REF OURS WAVDIR OUTDIR   (FTX_SET is passed through)
set -eu
ref=$1 ours=$2 wavs=$3 out=$4
bin=$(dirname "$0")/../../../target/release/examples
mkdir -p "$out"
python3 -I "$(dirname "$0")/score.py" "$ref" "$ours" -v | grep '^MISS' | grep -v ' a[0-9]$' > "$out/miss.txt" || true
: > "$out/explain.txt"
while read -r _ t snr dt f _ msg; do
    w=$(ls "$wavs"/*_"$t".wav)
    [ -f "$out/res_$t.wav" ] || FTX_RESIDUAL="$out/res_$t.wav" "$bin/ft8wav" "$w" > /dev/null 2>&1
    o=$("$bin/ft8probe" "$w" "$f" "$dt" "$msg" 2> /dev/null) || { echo "$snr nopack $msg" >> "$out/explain.txt"; continue; }
    r=$("$bin/ft8probe" "$out/res_$t.wav" "$f" "$dt" "$msg" 2> /dev/null)
    oo=$(echo "$o" | grep -c 'bp errors 0\|ok d=' || true)
    rr=$(echo "$r" | grep -c 'bp errors 0\|ok d=' || true)
    ns=$(echo "$o" | grep -o 'nsync [0-9]*' | head -1 | cut -d' ' -f2)
    echo "$snr orig=$oo resid=$rr nsync=$ns $f $dt $msg" >> "$out/explain.txt"
done < "$out/miss.txt"
awk '{c = ($2 == "nopack") ? "nopack" : (($2 != "orig=0") ? "decodable-orig" : (($3 != "resid=0") ? "decodable-resid" : "not-decodable")); n[c]++} END {for (k in n) print n[k], k}' "$out/explain.txt" | sort -rn
