#!/usr/bin/env bash
#
# SPDX-FileCopyrightText: OpenTalk GmbH <mail@opentalk.eu>
# SPDX-License-Identifier: EUPL-1.2

# This script automatically updates CHANGELOG.md to reflect the latest changes.
#
# Make sure to store a GitLab access token at `~/.gitlab_token`.

SCRIPT_DIR=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )
PROJECT_DIR=$( dirname "$SCRIPT_DIR" )
# Remove Unreleased section from Changelog

GITLAB_TOKEN_FILE=$HOME/.gitlab_token
GITLAB_REPO=opentalk/backend/libs/ics-chrono-tz

if [ -f "$GITLAB_TOKEN_FILE" ]; then
    echo "Using '$GITLAB_TOKEN_FILE' to authenticate with GitLab."
else
    echo "Please provide a GitLab token at '$GITLAB_TOKEN_FILE'."
    echo "You can create one here: https://git.opentalk.dev/-/user_settings/personal_access_tokens"
    echo "The scope should at least contain read_api."
fi

docker run --rm -it -v "$PROJECT_DIR":/app \
    -e GITLAB_REPO="$GITLAB_REPO" \
    -e GITLAB_API_URL="https://git.opentalk.dev/api/v4" \
    -e GITLAB_TOKEN="$(cat "$GITLAB_TOKEN_FILE")" \
    -u "$(id -u)":"$(id -g)" \
    git.opentalk.dev:5050/opentalk/tools/check-changelog:v0.3.0
