#!/usr/bin/env python3
"""Writes the registry tests/samples_registry.in and copies samples.json for samples.rs.

usage: samples_registry.py <samples.json> <sdk dir>

`perseid samples` names the type each language generates for every schema. The registry maps the
schema name to that type's codec, so samples.rs round-trips whatever the spec holds, without
knowing any model.
"""
import json
import os
import re
import shutil
import sys

samples, sdk = sys.argv[1:3]
with open(os.path.join(sdk, "Cargo.toml")) as cargo:
    crate = re.search(r'^name\s*=\s*"([^"]+)"', cargo.read(), re.M).group(1).replace("-", "_")
with open(samples) as source:
    models = json.load(source)

lines = [
    f"    ({json.dumps(schema)}, codec::<{crate}::models::{models[schema]['names']['rust']}> as Codec),"
    for schema in sorted(models)
]
tests = os.path.join(sdk, "tests")
os.makedirs(tests, exist_ok=True)
with open(os.path.join(tests, "samples_registry.in"), "w") as out:
    out.write("static CODECS: &[(&str, Codec)] = &[\n" + "\n".join(lines) + "\n];\n")
shutil.copy(samples, os.path.join(tests, "samples.json"))
print(f"{len(models)} models registered for {crate}")
