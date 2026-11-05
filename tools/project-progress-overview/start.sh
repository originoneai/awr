#!/bin/sh
# Launch Project Progress Overview; use the current directory as the project when omitted.
#   ./start.sh                     # Use the current directory as the AWR project.
#   ./start.sh /path/to/project    # Select a project directory.
set -e
DIR=$(cd "$(dirname "$0")" && pwd)
PROJECT=${1:-$(pwd)}
exec node "$DIR/server.js" --project "$PROJECT"
