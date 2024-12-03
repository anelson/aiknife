#!/bin/bash
#
# This script sets up a debugging proxy for an MCP server that monitors JSON-RPC traffic
#
# Usage: ./mcp-proxy.sh <mcp_server_command>
#
# For example, to run the MCP server that exposes your home directory, and monitor the traffic to and from that server:
# ./mcp-proxy.sh npx -y @modelcontextprotocol/server-filesystem ~/Dropbox
#
# The script will print instructions for connecting to the server, which will involve running `nc`
# with some command line that will point it to a UDS.
set -e # Exit on any error

if [ $# -eq 0 ]; then
	echo "Usage: $0 <mcp_server_command>"
	exit 1
fi

# Detect the operating system and netcat variant
OS="$(uname)"
NC_VERSION=$(nc -h 2>&1 || nc -help 2>&1 || true)

# Configure netcat based on characteristic command line options
if echo "$NC_VERSION" | grep -q "GNU netcat"; then
	echo "Detected GNU netcat"
	NC_LISTEN="nc -l -k -U" # -k keeps listening after client disconnects
elif echo "$NC_VERSION" | grep -q "\-k.*Keep inbound sockets open"; then
	# BSD netcat with -k support
	echo "Detected BSD netcat with -k support"
	NC_LISTEN="nc -l -k -U"
else
	# BSD netcat without -k support or unknown variant
	# We'll use a loop to maintain the connection
	echo "Detected BSD netcat (using persistent loop)"
	NC_LISTEN="while true; do nc -l -U"
	NC_LISTEN_END="; done"
fi

case "$OS" in
'Linux')
	SOCKET_DIR="/tmp/mcp_debug"
	NC_CONNECT="nc -U"
	;;
'Darwin')
	SOCKET_DIR="/var/tmp/mcp_debug"
	NC_CONNECT="nc -U"
	;;
*)
	echo "Unsupported operating system: $OS"
	exit 1
	;;
esac

# Single cleanup function that runs once
cleanup() {
	local exit_code=$?
	if [ -d "$SOCKET_DIR" ]; then
		echo "Cleaning up..."
		# Kill the entire process group to ensure we catch all background processes
		kill -TERM -$$
		rm -f "$SOCKET_DIR/mcp.sock" "$SOCKET_DIR/in_pipe" "$SOCKET_DIR/out_pipe"
		rmdir "$SOCKET_DIR" 2>/dev/null || true
	fi
	exit $exit_code
}

# Make sure we clean up when the script exits
trap cleanup EXIT INT TERM

# Set up our socket directory
mkdir -p "$SOCKET_DIR"
chmod 755 "$SOCKET_DIR"

# Clean up any existing sockets and pipes
rm -f "$SOCKET_DIR/mcp.sock" "$SOCKET_DIR/in_pipe" "$SOCKET_DIR/out_pipe"

# Create named pipes
mkfifo "$SOCKET_DIR/in_pipe"
mkfifo "$SOCKET_DIR/out_pipe"

# Timestamp function that handles both input and output with JSON formatting
timestamp_message() {
	local direction=$1
	while IFS= read -r line; do
		printf "[%s] %s\n" "$(date '+%H:%M:%S')" "$direction" >&2
		# Format JSON with jq and indent each line
		echo "$line" | jq '.' | sed 's/^/    /' >&2
		echo "$line" # Pass through the original message
	done
}

# Timestamp function for log messages written by the MCP server to stderr
# The spec encourages the use of JSON structured logging but it's not required
# so this doesn't make any assumptions about the format.
timestamp_log() {
	while IFS= read -r line; do
		printf "[%s] LOG >>> %s\n" "$(date '+%H:%M:%S')" "$line" >&2
	done
}

# Start the monitoring process that handles both directions
(
	# Put netcat in a subshell so we can handle its output properly
	(
		if [ -n "$NC_LISTEN_END" ]; then
			# For BSD netcat, use a loop
			eval "$NC_LISTEN \"$SOCKET_DIR/mcp.sock\" < \"$SOCKET_DIR/out_pipe\" | \
                timestamp_message \"IN\" > \"$SOCKET_DIR/in_pipe\" $NC_LISTEN_END"
		else
			# For GNU netcat, use -k
			$NC_LISTEN "$SOCKET_DIR/mcp.sock" <"$SOCKET_DIR/out_pipe" |
				timestamp_message "IN" >"$SOCKET_DIR/in_pipe"
		fi
	) &
	NC_PID=$!

	# Start the MCP server and connect it to our pipes, capturing stderr
	"$@" <"$SOCKET_DIR/in_pipe" 2> >(timestamp_log) | timestamp_message "OUT" >"$SOCKET_DIR/out_pipe"
) &
MONITOR_PID=$!

# Give netcat a moment to start listening
sleep 1

# Verify our background process is running
if ! kill -0 $MONITOR_PID 2>/dev/null; then
	echo "Failed to start monitoring process"
	exit 1
fi

echo "Debug proxy is running!"
echo "To connect to your MCP server, use:"
echo "$NC_CONNECT $SOCKET_DIR/mcp.sock"
echo "Monitoring output will appear in this terminal"
echo "Press Ctrl+C to stop the server"
echo

# Wait for the monitoring process to finish
wait $MONITOR_PID
