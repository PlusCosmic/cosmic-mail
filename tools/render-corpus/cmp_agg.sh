#!/bin/bash
# Two or more aggregate.py outputs side by side: one row per metric, one column per run.
# usage: cmp_agg.sh main.txt branch.txt [...]
f=$1; shift; cols=$(sed -n 2,31p "$f" | cut -c1-66)
for x in "$@"; do cols=$(paste -d'|' <(echo "$cols") <(sed -n 2,31p "$x" | cut -c56-66)); done; echo "$cols"
