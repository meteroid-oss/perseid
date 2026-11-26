#!/usr/bin/env bash
# Prototype of the builder's perseid step: overrides = ejected built-in reference + our templates.
set -euo pipefail
P=${PERSEID:-perseid}; T=$(cd "$(dirname "$0")/templates" && pwd); O=${1:-.perseid-docs}
for l in "${@:2}"; do
  "$P" eject "$l" templates/api_reference.md.jinja >/dev/null
  d=$O/templates/$l; mv "$d/api_reference.md.jinja" "$d/perseid_builtin_reference.jinja"
  cp "$T"/_common/* "$T/$l"/* "$d/"
done
