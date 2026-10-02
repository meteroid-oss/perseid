#!/usr/bin/env python3
"""Prepares a generated Go SDK for the sample round trips of samples_test.go.

usage: samples_registry.py <samples.json> <sdk dir>

`perseid samples` names the type each language generates for every schema. This writes, into the
SDK package, samples.json, samples_test.go (the test, in the package of the SDK) and
samples_registry_test.go, which maps every schema name to the codec of its Go type, so the test
round-trips whatever the spec holds without knowing any model.

The Go names `perseid samples` reports are plain upper camel case, while the SDK spells initialisms
the Go way (`URLSource` for `UrlSource`): a name that is not declared as is must match exactly one
declared type ignoring case.
"""
import glob
import json
import os
import re
import shutil
import sys

samples, sdk = sys.argv[1:3]
here = os.path.dirname(os.path.abspath(__file__))

sources = [path for path in sorted(glob.glob(os.path.join(sdk, "*.go"))) if not path.endswith("_test.go")]
text = "".join(open(path, encoding="utf-8").read() for path in sources)
package = re.search(r"^package (\w+)", text, re.M).group(1)
declared = re.findall(r"^type (\w+)", text, re.M)
by_lowercase = {}
for name in declared:
    by_lowercase.setdefault(name.lower(), []).append(name)

with open(samples, encoding="utf-8") as source:
    models = json.load(source)

lines = []
unresolved = []
for schema in sorted(models):
    name = models[schema]["names"]["go"]
    if name not in declared:
        candidates = by_lowercase.get(name.lower(), [])
        if len(candidates) != 1:
            unresolved.append(f"{schema} -> {name}: no Go type of that name ({candidates})")
            continue
        name = candidates[0]
    lines.append(f"\t{json.dumps(schema)}: samplesCodecOf[{name}],")
if unresolved:
    sys.exit("models without a Go type:\n  " + "\n  ".join(unresolved))

with open(os.path.join(sdk, "samples_registry_test.go"), "w", encoding="utf-8") as out:
    out.write(
        f"package {package}\n\n"
        "// samplesRegistry is the codec of the Go type of every schema, by schema name.\n"
        "var samplesRegistry = map[string]samplesCodec{\n" + "\n".join(lines) + "\n}\n"
    )
with open(os.path.join(here, "samples_test.go"), encoding="utf-8") as template:
    body = template.read().replace("package samples\n", f"package {package}\n", 1)
with open(os.path.join(sdk, "samples_test.go"), "w", encoding="utf-8") as out:
    out.write(body)
shutil.copy(samples, os.path.join(sdk, "samples.json"))
print(f"{len(models)} models registered for package {package}")
