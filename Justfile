set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

# Bastion MCP defaults aligned with ~/.config/opencode/opencode.json
host := env_var_or_default("BASTION_MCP_HOST", "127.0.0.1")
port := env_var_or_default("BASTION_MCP_PORT", "18765")
gateway := env_var_or_default("BASTION_GATEWAY_BIN", "target/release/bastion-gateway")
worker := env_var_or_default("BASTION_WORKER_BIN", "target/release/bastion-worker")
config := env_var_or_default("BASTION_CONFIG", "config/sandbox-gateway.toml")
config_dir := env_var_or_default("BASTION_CONFIG_DIR", ".bastion")
runtime_dir := env_var_or_default("BASTION_RUNTIME_DIR", ".bastion/runtime")
pid_file := runtime_dir + "/bastion-gateway.pid"
log_file := runtime_dir + "/bastion-gateway.log"

alias opencode-mcp-start := mcp-start
alias opencode-mcp-stop := mcp-stop
alias opencode-mcp-restart := mcp-restart
alias opencode-mcp-status := mcp-status
alias opencode-mcp-health := mcp-health

# Show available automations.
default:
	@just --list

# Compile all Bastion release binaries used by the MCP gateway and worker.
build-release:
	cargo build --release

# Start Bastion Gateway as a remote HTTP MCP server for OpenCode.
mcp-start:
	#!/usr/bin/env bash
	set -euo pipefail
	mkdir -p "{{runtime_dir}}"
	if [[ -f "{{pid_file}}" ]] && kill -0 "$(cat "{{pid_file}}")" 2>/dev/null; then
		echo "Bastion MCP already running: pid=$(cat "{{pid_file}}"), url=http://{{host}}:{{port}}"
		exit 0
	fi
	RUST_LOG="${RUST_LOG:-bastion=info}" nohup "{{gateway}}" \
		--transport http \
		--http-port "{{port}}" \
		--worker-binary "{{worker}}" \
		--config "{{config}}" \
		--config-dir "{{config_dir}}" \
		> "{{log_file}}" 2>&1 &
	echo "$!" > "{{pid_file}}"
	sleep 2
	just mcp-status

# Stop the Bastion Gateway MCP server started by `just mcp-start`.
mcp-stop:
	#!/usr/bin/env bash
	set -euo pipefail
	if [[ ! -f "{{pid_file}}" ]]; then
		echo "Bastion MCP is not running (missing {{pid_file}})."
		exit 0
	fi
	pid="$(cat "{{pid_file}}")"
	if kill -0 "$pid" 2>/dev/null; then
		kill "$pid"
		for _ in {1..30}; do
			if ! kill -0 "$pid" 2>/dev/null; then
				break
			fi
			sleep 0.1
		done
		if kill -0 "$pid" 2>/dev/null; then
			kill -9 "$pid"
		fi
		echo "Stopped Bastion MCP pid=$pid"
	else
		echo "Stale Bastion MCP pid file found for pid=$pid"
	fi
	rm -f "{{pid_file}}"

# Restart the Bastion Gateway MCP server.
mcp-restart: mcp-stop mcp-start

# Print process and TCP status for the Bastion Gateway MCP server.
mcp-status:
	#!/usr/bin/env bash
	set -euo pipefail
	if [[ -f "{{pid_file}}" ]] && kill -0 "$(cat "{{pid_file}}")" 2>/dev/null; then
		echo "Bastion MCP running: pid=$(cat "{{pid_file}}"), url=http://{{host}}:{{port}}"
		python3 -c "import socket; h='{{host}}'; p=int('{{port}}'); s=socket.socket(); s.settimeout(1); s.connect((h, p)); print(f'TCP check: OK ({h}:{p})'); s.close()" \
		|| echo "TCP check: FAIL ({{host}}:{{port}})"
	else
		echo "Bastion MCP not running. Start with: just mcp-start"
		exit 1
	fi

# Call MCP initialize + sandbox_health over Streamable HTTP.
mcp-health:
	python3 scripts/mcp-health.py "{{host}}" "{{port}}"

# Follow the gateway log created by `just mcp-start`.
mcp-logs:
	mkdir -p "{{runtime_dir}}"
	touch "{{log_file}}"
	tail -f "{{log_file}}"