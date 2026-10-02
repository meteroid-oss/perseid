#!/bin/bash
# java only into out/<name>-java
F=/tmp/claude-0/-home-user-perseid/dfe4da85-da76-5610-9dab-d14c668ed481/scratchpad/fuzz
P=/home/user/perseid/target/release/perseid
spec=$(realpath "$1"); name=$(basename "$spec" .yaml)-java
out=$F/out/$name; rm -rf "$out"; mkdir -p "$out"; cd "$out"
$P init --sdks java --spec "$spec" > init.log 2>&1
[ -f "${spec%.yaml}.toml" ] && cat "${spec%.yaml}.toml" >> perseid.toml
if ! $P generate java > gen-java.log 2>&1; then echo "$name: java:GENFAIL"; exit; fi
(cd java && gradle build --no-daemon -q 2>&1 | grep -v JAVA_TOOL_OPTIONS; exit ${PIPESTATUS[0]}) > java.log 2>&1 && echo "$name: java:ok" || echo "$name: java:FAIL"
