#!/usr/bin/env bash
# Writes `token=` to $GITHUB_OUTPUT: a token of the hosted perseid App for REPOSITORIES (names or
# owner/name, comma or space separated, OWNER for bare names), asked of the broker with this job's
# OIDC token. With OPTIONAL=true, a job that can't get one goes on without it.
set -euo pipefail

broker="${PERSEID_BROKER:-https://perseid.meteroid.com/broker}"
docs="https://github.com/meteroid-oss/perseid/blob/main/docs/ci.md#credentials"

give_up() {
  if [ "${OPTIONAL:-false}" = true ]; then
    echo "::notice::$1"
    exit 0
  fi
  echo "::error::$1"
  exit 1
}

[ -n "${ACTIONS_ID_TOKEN_REQUEST_URL:-}" ] || give_up \
  "no credential: give the job \`permissions: id-token: write\` for the perseid App, or set up your own App or token ($docs)"

oidc=$(curl -sSf -H "Authorization: Bearer $ACTIONS_ID_TOKEN_REQUEST_TOKEN" \
  "$ACTIONS_ID_TOKEN_REQUEST_URL&audience=perseid" | jq -r .value)
body=$(jq -cn --arg repos "$REPOSITORIES" --arg owner "${OWNER:-}" --argjson permissions "${PERMISSIONS:-null}" '
  {repositories: ($repos | split("[ ,\n]+"; null) | map(select(. != "") | if contains("/") then . else "\($owner)/\(.)" end))}
  + if $permissions then {permissions: $permissions} else {} end')
response=$(curl -sS -w '\n%{http_code}' -X POST "$broker/token" \
  -H "Authorization: Bearer $oidc" -H 'Content-Type: application/json' -d "$body") ||
  give_up "the perseid App's broker at $broker is unreachable"
status=${response##*$'\n'}
reply=${response%$'\n'*}
[ "$status" = 200 ] || give_up "the perseid App: $(jq -r '.error // empty' <<< "$reply" 2>/dev/null || true) (HTTP $status, $docs)"

token=$(jq -r .token <<< "$reply")
echo "::add-mask::$token"
echo "token=$token" >> "$GITHUB_OUTPUT"
