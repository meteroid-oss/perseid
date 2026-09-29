"""Prepare OpenAPI 3.0 input for the inherited 3.1 code generator.

The checked-in source spec is never modified. Unsupported transport operations
are recorded explicitly in codegen/coverage.json.
"""

import copy
import json
import re
import unicodedata
from pathlib import Path

METHODS = {"get", "post", "put", "patch", "delete", "head", "options", "trace"}


def prepare(
    source: Path,
    destination: Path,
    *,
    report: Path,
    exclude_unsupported: bool = False,
    normalize_tags: bool = False,
) -> None:
    spec = json.loads(source.read_text())
    spec["openapi"] = "3.1.0"
    schemas = spec["components"]["schemas"]
    excluded = []
    included = []

    def name(value):
        return "".join(
            part[:1].upper() + part[1:] for part in re.findall(r"[A-Za-z0-9]+", value)
        )

    def promote(schema, suggested):
        if "$ref" in schema:
            return schema
        key = name(suggested)
        base = key
        counter = 2
        while key in schemas and schemas[key] != schema:
            key = f"{base}{counter}"
            counter += 1
        schemas[key] = schema
        return {"$ref": f"#/components/schemas/{key}"}

    for path, item in spec["paths"].items():
        for method, op in list(item.items()):
            if method not in METHODS:
                continue
            reason = None
            content = op.get("requestBody", {}).get("content", {})
            if any(
                t not in {"application/json", "application/x-www-form-urlencoded"}
                for t in content
            ):
                reason = "Upload request body: " + ", ".join(content)
            responses = op.get("responses", {})
            if any(
                "text/event-stream" in r.get("content", {}) for r in responses.values()
            ):
                reason = "Server-sent event stream requires a streaming transport"
            successes = {c: r for c, r in responses.items() if c.startswith("2")}
            if not successes:
                reason = "No documented 2xx response (redirect or protocol upgrade)"
            if reason and not exclude_unsupported:
                raise ValueError(f"Unsupported operation {op['operationId']}: {reason}")
            if reason:
                excluded.append(
                    {
                        "method": method.upper(),
                        "path": path,
                        "operation_id": op["operationId"],
                        "reason": reason,
                    }
                )
                del item[method]
                continue
            included.append(
                {
                    "method": method.upper(),
                    "path": path,
                    "operation_id": op["operationId"],
                }
            )
            # Tags include Unicode and Rust module paths. Keep stable ASCII names.
            if normalize_tags:
                op["tags"] = [
                    unicodedata.normalize(
                        "NFKD", " ".join(t.split("::")[-2:]).removeprefix("super ")
                    )
                    .encode("ascii", "ignore")
                    .decode()
                    for t in op.get("tags", [])
                ]
            # Redirects are handled by the HTTP status error path, not as success bodies.
            op["responses"] = {
                c: r for c, r in responses.items() if c.startswith(("2", "4", "5"))
            }
            for media, body in content.items():
                body["schema"] = promote(
                    body.get("schema", {}), op["operationId"] + "_request"
                )
            for code, response in op["responses"].items():
                body = response.get("content", {}).get("application/json")
                if body is not None:
                    body["schema"] = promote(
                        body.get("schema", {}), op["operationId"] + "_response_" + code
                    )

    def normalize(schema, context):
        if not isinstance(schema, dict):
            return schema
        schema = copy.deepcopy(schema)
        if (
            schema.get("type") == "object"
            and not schema.get("properties")
            and not any(k in schema for k in ("allOf", "oneOf", "anyOf"))
        ):
            schema.setdefault("additionalProperties", True)
        # A single allOf reference is an alias, often decorated with nullable.
        if len(schema.get("allOf", [])) == 1 and "$ref" in schema["allOf"][0]:
            schema.update(schema.pop("allOf")[0])
        for field, value in list(schema.get("properties", {}).items()):
            value = normalize(value, context + "_" + field)
            if value.get("type") == "object" and value.get("properties"):
                nullable = value.get("nullable", False)
                value = promote(value, context + "_" + field)
                if nullable:
                    value["nullable"] = True
            schema["properties"][field] = value
        if isinstance(schema.get("items"), dict):
            value = normalize(schema["items"], context + "_item")
            if value.get("properties") or (
                value.get("enum") and len(value["enum"]) > 1
            ):
                value = promote(value, context + "_item")
            schema["items"] = value
        if isinstance(schema.get("additionalProperties"), dict):
            value = normalize(schema["additionalProperties"], context + "_value")
            if value.get("properties"):
                value = promote(value, context + "_value")
            schema["additionalProperties"] = value
        # Expand object allOf variants so tagged enum fields remain strongly typed.
        if "allOf" in schema and len(schema["allOf"]) > 1:
            merged = {"type": "object", "properties": {}, "required": []}
            for part in schema["allOf"]:
                if "$ref" in part:
                    part = schemas[part["$ref"].rsplit("/", 1)[-1]]
                if part.get("type") != "object":
                    break
                merged["properties"].update(part.get("properties", {}))
                merged["required"].extend(part.get("required", []))
            else:
                schema.pop("allOf")
                schema.update(merged)
        for composition in ("allOf", "oneOf", "anyOf"):
            if composition in schema:
                schema[composition] = [
                    normalize(s, context + "_" + str(i))
                    for i, s in enumerate(schema[composition])
                ]
        if "oneOf" in schema and "discriminator" not in schema:
            variants = schema["oneOf"]
            candidates = []
            for variant in variants:
                candidates.append(
                    {
                        k
                        for k, v in variant.get("properties", {}).items()
                        if v.get("type") == "string" and len(v.get("enum", [])) == 1
                    }
                )
            common = set.intersection(*candidates) if candidates else set()
            if len(common) == 1:
                schema["discriminator"] = {"propertyName": common.pop()}
        if len(schema.get("oneOf", [])) == 1 and "discriminator" not in schema:
            variant = schema.pop("oneOf")[0]
            schema.update(variant)
        if schema.get("additionalProperties") is False and schema.get("properties"):
            del schema["additionalProperties"]
        return schema

    processed = set()
    while pending := set(schemas) - processed:
        for key in sorted(pending):
            schemas[key] = normalize(schemas[key], key)
            processed.add(key)
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(json.dumps(spec, indent=2) + "\n")
    report.write_text(
        json.dumps(
            {
                "source": str(source),
                "included_operations": len(included),
                "excluded_operations": excluded,
            },
            indent=2,
        )
        + "\n"
    )
    print(
        f"Prepared {len(included)} operations; {len(excluded)} excluded (recorded in coverage report)",
        flush=True,
    )
