#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
MODULE_ROOT="${EXPERIMENTS_MODULE_ROOT:-$SCRIPT_DIR}"
SETUP_SCRIPT="${EXPERIMENTS_SETUP_SCRIPT:-$SCRIPT_DIR/setup.sh}"
CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
WASMER_DIR="${WASMER_DIR:-$HOME/.wasmer}"
export PATH="$CARGO_HOME/bin:$WASMER_DIR/bin:$PATH"

usage() {
    cat <<EOF
Usage: bash experiments/run.sh [--detach] [--reset-network]

Choose experiments by entering their numbers separated by commas, in the order
they should run. Enter 6 to run all five in the standard order, or 7 to
reevaluate reports from saved results without rerunning experiments.

  1) Runtime
  2) Empty block
  3) Startup
  4) Compliance (Hive ethereum/engine)
  5) Size comparison
  6) All experiments
  7) Evaluate saved results

For each selected run, press Enter to use the module's defaults or choose
custom values before the shared setup starts.

  --detach                 Continue the selected suite in a persistent session
  --reset-network          Allow runtime, empty-block, and startup runs to clear generated network data
  --attach SESSION_ID      Reattach to a session's console log
  --stop SESSION_ID        Stop a detached session and its child processes
EOF
}

prompt_value() {
    local variable="$1" label="$2" default="$3" kind="$4" value
    while true; do
        if ! IFS= read -r -p "$label [$default]: " value; then
            echo "Error: input ended while configuring $label." >&2
            return 2
        fi
        value="${value:-$default}"
        case "$kind" in
            positive)
                [[ "$value" =~ ^[1-9][0-9]*$ ]] || { echo "Enter a positive whole number." >&2; continue; }
                ;;
            nonnegative)
                [[ "$value" =~ ^[0-9]+$ ]] || { echo "Enter a nonnegative whole number." >&2; continue; }
                ;;
            positive_decimal)
                [[ "$value" =~ ^[0-9]+([.][0-9]+)?$ ]] && awk "BEGIN { exit !($value > 0) }" || {
                    echo "Enter a positive number." >&2
                    continue
                }
                ;;
            text)
                [[ -n "$value" ]] || { echo "A value is required." >&2; continue; }
                ;;
            optional_text)
                ;;
        esac
        printf -v "$variable" '%s' "$value"
        return 0
    done
}

prompt_yes_no() {
    local variable="$1" label="$2" default="$3" value
    while true; do
        if ! IFS= read -r -p "$label [$default]: " value; then
            echo "Error: input ended while configuring $label." >&2
            return 2
        fi
        value="${value:-$default}"
        case "${value,,}" in
            y|yes) printf -v "$variable" 'true'; return 0 ;;
            n|no) printf -v "$variable" 'false'; return 0 ;;
            *) echo "Enter yes or no." >&2 ;;
        esac
    done
}

prompt_customize() {
    local experiment="$1" value
    echo "Default $experiment configuration: $2"
    while true; do
        if ! IFS= read -r -p 'Customize these values? [y/N]: ' value; then
            echo "Error: input ended while configuring $experiment." >&2
            return 2
        fi
        case "${value,,}" in
            y|yes) return 0 ;;
            ''|n|no) echo "Using $experiment defaults."; return 1 ;;
            *) echo "Enter yes or no." >&2 ;;
        esac
    done
}

run_logged() {
    local log_file="$1" description="$2" status
    shift 2
    mkdir -p "$(dirname "$log_file")"
    printf '\n[%s] %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$description" | tee -a "$log_file"
    if "$@" 2>&1 | tee -a "$log_file"; then
        status=0
    else
        status="${PIPESTATUS[0]}"
    fi
    printf '[%s] Exit status: %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$status" | tee -a "$log_file"
    return "$status"
}

valid_session_id() {
    [[ "$1" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$ ]]
}

attach_session() {
    local session_id="$1" session_dir="$EXPERIMENTS_OUTPUT_ROOT/logs/$1"
    if ! valid_session_id "$session_id" || [[ ! -f "$session_dir/runner.log" ]]; then
        echo "Error: no experiment console log found for session '$session_id' at '$session_dir/runner.log'." >&2
        return 2
    fi
    if [[ -f "$session_dir/runner.status" ]]; then
        cat "$session_dir/runner.log"
    else
        tail -n +1 -f "$session_dir/runner.log"
    fi
}

stop_session() {
    local session_id="$1" session_dir="$EXPERIMENTS_OUTPUT_ROOT/logs/$1" pid command_line process_session
    if ! valid_session_id "$session_id" || [[ ! -f "$session_dir/runner.pid" ]]; then
        echo "Error: no detached session found for '$session_id'." >&2
        return 2
    fi
    if [[ -f "$session_dir/runner.status" ]]; then
        echo "Session $session_id has already finished."
        return 0
    fi
    pid="$(<"$session_dir/runner.pid")"
    if ! [[ "$pid" =~ ^[0-9]+$ ]] || ! kill -0 "$pid" 2>/dev/null; then
        echo "Error: session $session_id is not running (stale PID file)." >&2
        return 1
    fi
    if [[ -r "/proc/$pid/cmdline" ]]; then
        command_line="$(tr '\0' ' ' < "/proc/$pid/cmdline")"
        if [[ "$command_line" != *"--resume $session_id"* ]]; then
            echo "Error: PID $pid does not belong to session $session_id." >&2
            return 1
        fi
    fi
    process_session="$(ps -o sid= -p "$pid" 2>/dev/null | tr -d '[:space:]')"
    if [[ "$process_session" != "$pid" ]]; then
        echo "Error: PID $pid is not the detached session leader for $session_id." >&2
        return 1
    fi
    if ! kill -TERM -- "-$pid" 2>/dev/null; then
        echo "Error: could not stop detached session $session_id." >&2
        return 1
    fi
    echo "Sent stop signal to session $session_id."
}

validate_csv_choices() {
    local value="$1" allowed="$2" item candidate found
    local -A list_seen=()
    IFS=',' read -r -a items <<< "$value"
    (( ${#items[@]} > 0 )) || return 1
    for item in "${items[@]}"; do
        item="${item//[[:space:]]/}"
        [[ -n "$item" && -z "${list_seen[$item]:-}" ]] || return 1
        found=false
        IFS=',' read -r -a candidates <<< "$allowed"
        for candidate in "${candidates[@]}"; do
            if [[ "$item" == "$candidate" ]]; then found=true; break; fi
        done
        [[ "$found" == true ]] || return 1
        list_seen[$item]=1
    done
}

validate_workload_types() {
    local value="$1" workload
    local -A workload_seen=()
    runtime_workloads=()
    IFS=',' read -r -a workload_items <<< "$value"
    (( ${#workload_items[@]} > 0 )) || return 1
    for workload in "${workload_items[@]}"; do
        workload="${workload//[[:space:]]/}"
        [[ "$workload" =~ ^[A-Za-z0-9._/-]+$ ]] || return 1
        [[ -z "${workload_seen[$workload]:-}" ]] || return 1
        workload_seen[$workload]=1
        runtime_workloads+=("$workload")
    done
}

validate_counts() {
    local value="$1" item normalized
    local -A list_seen=()
    has_nonzero_count=false
    IFS=',' read -r -a items <<< "$value"
    (( ${#items[@]} > 0 )) || return 1
    for item in "${items[@]}"; do
        item="${item//[[:space:]]/}"
        [[ "$item" =~ ^[0-9]{1,9}$ ]] || return 1
        normalized=$((10#$item))
        [[ -z "${list_seen[$normalized]:-}" ]] || return 1
        list_seen[$normalized]=1
        if (( normalized > 0 )); then has_nonzero_count=true; fi
    done
}

configure_module() {
    local experiment="$1" customize result reset_network="${RESET_NETWORK:-false}"
    local has_nonzero_count=false eb_clients eb_clients_normalized eb_client eb_workload eb_reset_network startup_configs_default startup_wasix_eth_root startup_wasix_wasm_default
    local -n module_args="${experiment//-/_}_args"
    module_args=()

    case "$experiment" in
        runtime)
            if prompt_customize runtime '2 native, 2 WASIX, 0 Geth, 0 Reth nodes; exp/uber and exp/uber_v2 workloads; 4 TPS; 4 accounts; 300 seconds; 1 iteration'; then
                prompt_value linux_nodes 'Native/Linux nodes' 2 nonnegative || return $?
                prompt_value wasix_nodes 'WASIX nodes' 2 nonnegative || return $?
                prompt_value geth_nodes 'Geth nodes' 0 nonnegative || return $?
                prompt_value reth_nodes 'Reth nodes' 0 nonnegative || return $?
                if (( 10#$linux_nodes + 10#$wasix_nodes + 10#$geth_nodes + 10#$reth_nodes == 0 )); then
                    echo 'At least one runtime node is required.' >&2
                    return 2
                fi
                while true; do
                    prompt_value workload_type 'Contender workload scenarios (comma-separated)' 'exp/uber,exp/uber_v2' text || return $?
                    if validate_workload_types "$workload_type"; then break; fi
                    echo 'Use unique scenario paths with letters, numbers, dots, underscores, slashes, or hyphens.' >&2
                done
                prompt_value accounts 'Spamming accounts per workload node' 4 positive || return $?
                prompt_value tps 'Target TPS per workload node' 4 positive || return $?
                prompt_value duration 'Workload duration in seconds' 300 positive || return $?
                prompt_value wait_duration 'Post-workload collection wait in seconds' 1800 nonnegative || return $?
                prompt_value chain_id 'Network chain ID' 12345 positive || return $?
                prompt_value runtime_iterations 'Number of independent runtime iterations' 1 positive || return $?
                if [[ "$RESET_NETWORK" != true ]]; then
                    prompt_yes_no reset_network 'Allow clearing generated network state' no || return $?
                fi
                module_args=(--linux-nodes "$linux_nodes" --wasix-nodes "$wasix_nodes"
                    --geth-nodes "$geth_nodes" --reth-nodes "$reth_nodes"
                    --accounts "$accounts" --tps "$tps"
                    --duration "$duration" --wait-duration "$wait_duration" --chain-id "$chain_id")
            else
                result=$?
                (( result == 1 )) || return "$result"
                runtime_iterations=1
                runtime_workloads=(exp/uber exp/uber_v2)
            fi
            [[ "$reset_network" == true ]] && module_args+=(--reset-network)
            ;;
        empty-block)
            eb_reset_network="$reset_network"
            source "$REPO_ROOT/experiments/empty-block/defaults.sh"
            empty_block_clients=(native wasix geth reth)
            if prompt_customize empty-block 'native, wasix, geth, and reth clients; counts 0,1,5,10,20,50,100; 3 samples per count; max 5 attempts'; then
                while true; do
                    prompt_value eb_clients 'Clients (comma-separated: native, wasix, geth, reth)' 'native,wasix,geth,reth' text || return $?
                    eb_clients="${eb_clients//[[:space:]]/}"
                    IFS=',' read -r -a empty_block_clients <<< "$eb_clients"
                    eb_clients_normalized=""
                    for eb_client in "${empty_block_clients[@]}"; do
                        if [[ "$eb_client" == linux ]]; then eb_client=native; fi
                        if [[ -n "$eb_clients_normalized" ]]; then eb_clients_normalized+=,; fi
                        eb_clients_normalized+="$eb_client"
                    done
                    if validate_csv_choices "$eb_clients_normalized" 'native,wasix,geth,reth'; then
                        IFS=',' read -r -a empty_block_clients <<< "$eb_clients_normalized"
                        break
                    fi
                    echo 'Choose unique clients from native, wasix, geth, and reth.' >&2
                done
                prompt_value eb_chain_id 'Network chain ID' 23456 positive || return $?
                while true; do
                    prompt_value eb_counts 'Transactions per block-count group (comma-separated)' '0,1,5,10,20,50,100' text || return $?
                    if validate_counts "$eb_counts"; then break; fi
                    echo 'Use unique, comma-separated nonnegative integers.' >&2
                done
                prompt_value eb_blocks 'Samples/iterations per transaction count' 3 positive || return $?
                prompt_value eb_attempts 'Maximum attempts per transaction count' 5 positive || return $?
                prompt_value eb_receipt_timeout 'Receipt timeout in seconds' 180 positive || return $?
                prompt_value eb_block_timeout 'Block timeout in seconds' 120 positive || return $?
                prompt_value eb_poll_interval 'RPC poll interval in seconds' 1 positive_decimal || return $?
                if [[ "$reset_network" != true ]]; then
                    prompt_yes_no eb_reset_network 'Allow clearing generated network state' no || return $?
                fi
                while true; do
                    prompt_value eb_workload 'Contract workload (none, counter, dota, uber, uber_v2)' none text || return $?
                    case "$eb_workload" in none|counter|dota|uber|uber_v2) break ;; esac
                    echo 'Choose none, counter, dota, uber, or uber_v2.' >&2
                done
                module_args=(--chain-id "$eb_chain_id" --counts "$eb_counts"
                    --blocks-per-count "$eb_blocks" --max-attempts "$eb_attempts"
                    --receipt-timeout "$eb_receipt_timeout" --block-timeout "$eb_block_timeout"
                    --poll-interval "$eb_poll_interval")
                [[ "$eb_workload" == none ]] || module_args+=(--workload "$eb_workload")
            else
                result=$?
                (( result == 1 )) || return "$result"
            fi
            [[ "$eb_reset_network" == true ]] && module_args+=(--reset-network)
            if [[ -z "${EB_PRIVATE_KEY:-}" && ( ${#module_args[@]} -eq 0 || "$has_nonzero_count" == true ) ]]; then
                EB_PRIVATE_KEY="$EMPTY_BLOCK_DEFAULT_PRIVATE_KEY"
            fi
            ;;
        startup)
            startup_configs_default='geth,reth,nexus-linux,nexus-wasix-docker,nexus-wasmer'
            startup_wasix_eth_root="${EXPERIMENTS_WASIX_ETH_ROOT:-$REPO_ROOT/wasix_eth}"
            startup_wasix_wasm_default="${NEXUS_WASIX_WASM:-$startup_wasix_eth_root/target/wasm32-wasmer-wasi/release/wasix_eth.wasi.wasm}"
            if prompt_customize startup "$startup_configs_default; 5 iterations; readiness timeout 60 seconds; chain ID 12345"; then
                while true; do
                    prompt_value startup_configs 'Configurations (comma-separated)' "$startup_configs_default" text || return $?
                    if validate_csv_choices "$startup_configs" 'geth,reth,nexus-linux,nexus-wasix-docker,nexus-wasmer'; then break; fi
                    echo 'Choose unique startup configurations from the displayed list.' >&2
                done
                startup_configs="${startup_configs//[[:space:]]/}"
                prompt_value startup_iterations 'Iterations per configuration' 5 positive || return $?
                prompt_value startup_chain_id 'Isolated genesis chain ID' 12345 positive || return $?
                prompt_value startup_timeout 'Readiness timeout in seconds' 60 positive_decimal || return $?
                prompt_value startup_interval 'System metrics interval in seconds' 0.2 positive_decimal || return $?
                prompt_value geth_image 'Geth image' 'ethereum/client-go@sha256:798b7eb1bcef6d4be7576232beea63bf291450f48b025dc8fb5c6e37840e4364' text || return $?
                prompt_value reth_image 'Reth image' 'ghcr.io/paradigmxyz/reth:latest' text || return $?
                prompt_value nexus_linux_image 'NEXUS Linux image' 'wasix-eth-linux:latest' text || return $?
                prompt_value nexus_wasix_image 'NEXUS WASIX Docker image' 'wasix-eth-wasix:latest' text || return $?
                prompt_value nexus_wasix_wasm 'NEXUS WASIX module path' "$startup_wasix_wasm_default" optional_text || return $?
                prompt_value wasmer_bin 'Wasmer executable' wasmer text || return $?
                module_args=(--configs "$startup_configs" --iterations "$startup_iterations"
                    --chain-id "$startup_chain_id" --timeout "$startup_timeout"
                    --metrics-interval "$startup_interval" --geth-image "$geth_image"
                    --reth-image "$reth_image" --nexus-linux-image "$nexus_linux_image"
                    --nexus-wasix-image "$nexus_wasix_image" --wasmer-bin "$wasmer_bin")
                [[ -n "$nexus_wasix_wasm" ]] && module_args+=(--nexus-wasix-wasm "$nexus_wasix_wasm")
                if [[ ",$startup_configs," == *,nexus-wasmer,* && -z "$nexus_wasix_wasm" ]]; then
                    echo 'The nexus-wasmer configuration requires a NEXUS WASIX module path.' >&2
                    return 2
                fi
            else
                result=$?
                (( result == 1 )) || return "$result"
            fi
            [[ "$RESET_NETWORK" == true ]] && module_args+=(--reset-network)
            ;;
        compliance)
            if prompt_customize compliance 'both custom Hive clients; hive-custom:latest; parallelism 20; timeout 3600 seconds; build images'; then
                while true; do
                    prompt_value hive_clients 'Hive clients (comma-separated)' 'wasix-r-eth,wasix-w-eth' text || return $?
                    if validate_csv_choices "$hive_clients" 'wasix-r-eth,wasix-w-eth'; then break; fi
                    echo 'Choose unique clients: wasix-r-eth and/or wasix-w-eth.' >&2
                done
                hive_clients="${hive_clients//[[:space:]]/}"
                prompt_value hive_image 'Hive runner image' 'hive-custom:latest' text || return $?
                prompt_value hive_parallelism 'Hive parallelism' 20 positive || return $?
                prompt_value hive_timeout 'Per-client timeout in seconds' 3600 positive || return $?
                prompt_yes_no skip_base_build 'Skip custom client image builds (verify existing images)' no || return $?
                prompt_yes_no skip_hive_build 'Skip Hive runner image build (verify existing image)' no || return $?
                module_args=(--clients "$hive_clients" --hive-image "$hive_image"
                    --parallelism "$hive_parallelism" --timeout "$hive_timeout")
                [[ "$skip_base_build" == true ]] && module_args+=(--skip-base-build)
                [[ "$skip_hive_build" == true ]] && module_args+=(--skip-hive-build)
            else
                result=$?
                (( result == 1 )) || return "$result"
            fi
            ;;
        size)
            size_sources_default="wasix-eth=$REPO_ROOT/wasix_eth"
            [[ -n "${GETH_SOURCE_DIR:-}" ]] && size_sources_default+=",geth=$GETH_SOURCE_DIR"
            [[ -n "${RETH_SOURCE_DIR:-}" ]] && size_sources_default+=",reth=$RETH_SOURCE_DIR"
            size_binaries_default="wasix-eth-native=${WASIX_ETH_NATIVE_BINARY:-$REPO_ROOT/wasix_eth/target/release/wasix_eth},wasix-eth-wasix=${WASIX_ETH_WASIX_BINARY:-$REPO_ROOT/wasix_eth/target/wasm32-wasmer-wasi/release/wasix_eth.wasi.wasm}"
            [[ -n "${GETH_BINARY:-}" ]] && size_binaries_default+=",geth=$GETH_BINARY"
            [[ -n "${RETH_BINARY:-}" ]] && size_binaries_default+=",reth=$RETH_BINARY"
            if prompt_customize size "source inputs: $size_sources_default; binary inputs: $size_binaries_default"; then
                prompt_value size_sources 'Source inputs LABEL=DIR (comma-separated)' "$size_sources_default" text || return $?
                prompt_value size_binaries 'Binary inputs LABEL=FILE (comma-separated)' "$size_binaries_default" text || return $?
                local -a source_entries=()
                IFS=',' read -r -a source_entries <<< "$size_sources"
                for entry in "${source_entries[@]}"; do
                    [[ "$entry" == *=* && -n "${entry%%=*}" && -n "${entry#*=}" ]] || { echo 'Each source must be LABEL=DIR.' >&2; return 2; }
                    module_args+=(--source "$entry")
                done
                local -a binary_entries=()
                IFS=',' read -r -a binary_entries <<< "$size_binaries"
                for entry in "${binary_entries[@]}"; do
                    [[ "$entry" == *=* && -n "${entry%%=*}" && -n "${entry#*=}" ]] || { echo 'Each binary must be LABEL=FILE.' >&2; return 2; }
                    module_args+=(--binary "$entry")
                done
            else
                result=$?
                (( result == 1 )) || return "$result"
            fi
            ;;
    esac
    return 0
}

EXPERIMENTS_OUTPUT_ROOT="${EXPERIMENTS_OUTPUT_ROOT:-$REPO_ROOT/experiment-results}"
export EXPERIMENTS_OUTPUT_ROOT

DETACH_MODE=false
RESET_NETWORK=false
RESUME_SESSION=""
CONTROL_ACTION=""
CONTROL_SESSION=""
while [[ "$#" -gt 0 ]]; do
    case "$1" in
        --help|-h) usage; exit 0 ;;
        --detach)
            [[ "$DETACH_MODE" == false ]] || { echo "Error: --detach may only be specified once." >&2; exit 2; }
            DETACH_MODE=true
            shift
            ;;
        --reset-network)
            RESET_NETWORK=true
            shift
            ;;
        --attach|--stop|--resume)
            [[ -z "$CONTROL_ACTION" && "$#" -eq 2 ]] || { usage >&2; exit 2; }
            CONTROL_ACTION="$1"
            CONTROL_SESSION="$2"
            shift 2
            ;;
        *)
            echo "Error: unknown runner option: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

if [[ -n "$CONTROL_ACTION" ]]; then
    [[ "$DETACH_MODE" == false && "$RESET_NETWORK" == false ]] || {
        echo "Error: session control options cannot be combined with --detach or --reset-network." >&2
        exit 2
    }
    case "$CONTROL_ACTION" in
        --attach)
            attach_session "$CONTROL_SESSION"
            exit $?
            ;;
        --stop)
            stop_session "$CONTROL_SESSION"
            exit $?
            ;;
        --resume)
            [[ "$CONTROL_SESSION" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$ ]] || {
                echo "Error: invalid detached session ID." >&2
                exit 2
            }
            RESUME_SESSION="$CONTROL_SESSION"
            ;;
    esac
fi

if [[ "$DETACH_MODE" == true ]] && { ! command -v setsid >/dev/null 2>&1 || ! command -v nohup >/dev/null 2>&1; }; then
    echo "Error: detached sessions require the setsid and nohup commands." >&2
    exit 1
fi

declare -a labels=(runtime empty-block startup compliance size)
declare -a selected=()
declare -a runtime_args=() empty_block_args=() startup_args=() compliance_args=() size_args=()
declare -a runtime_workloads=(exp/uber exp/uber_v2)
declare -a empty_block_clients=(native wasix geth reth)
runtime_iterations=1
mode=run

if [[ -n "$RESUME_SESSION" ]]; then
    RUN_LOG_DIR="$EXPERIMENTS_OUTPUT_ROOT/logs/$RESUME_SESSION"
    STATE_FILE="$RUN_LOG_DIR/runner-state.sh"
    if [[ ! -f "$STATE_FILE" ]]; then
        echo "Error: detached session state was not found for '$RESUME_SESSION'." >&2
        exit 1
    fi
    exec >> "$RUN_LOG_DIR/runner.log" 2>&1
    record_session_status() {
        local status=$?
        printf '%s\n' "$status" > "$RUN_LOG_DIR/runner.status"
        rm -f "$STATE_FILE"
    }
    trap record_session_status EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
    source "$STATE_FILE"
    export EXPERIMENTS_SELECTED EXPERIMENT_RUN_ID BASE_EXPERIMENT_RUN_ID
else
    LOG_SESSION_ID="$(date -u +%Y%m%dT%H%M%SZ)-$$"
    RUN_LOG_DIR="$EXPERIMENTS_OUTPUT_ROOT/logs/$LOG_SESSION_ID"
    mkdir -p "$RUN_LOG_DIR"
    exec > >(tee -a "$RUN_LOG_DIR/runner.log")
    OUTPUT_TEE_PID=$!
    exec 2> >(tee -a "$RUN_LOG_DIR/runner.log" >&2)
    ERROR_TEE_PID=$!
    flush_console_tees() {
        local status=$?
        trap - EXIT
        exec 1>&- 2>&-
        wait "$OUTPUT_TEE_PID" "$ERROR_TEE_PID" 2>/dev/null || true
        return "$status"
    }
    trap flush_console_tees EXIT
    echo "Suite log directory: $RUN_LOG_DIR"
fi

if [[ -z "$RESUME_SESSION" ]]; then
usage
while true; do
    if [[ "$mode" == evaluate ]]; then
        prompt='Experiments to evaluate (1-5, comma-separated, or 6 for all): '
    else
        prompt='Selection: '
    fi
    if ! IFS= read -r -p "$prompt" selection; then
        echo "Error: no selection was provided." >&2
        exit 2
    fi
    selection="${selection//[[:space:]]/}"
    if [[ -z "$selection" ]]; then
        echo "Please select at least one experiment or action." >&2
        continue
    fi
    if [[ "$mode" == run && "$selection" == "7" ]]; then
        mode=evaluate
        echo "Evaluation mode: reports will be regenerated from saved results; experiments will not be rerun."
        continue
    fi
    if [[ "$selection" == "6" || "${selection,,}" == "all" ]]; then
        selected=("${labels[@]}")
        break
    fi

    IFS=',' read -r -a choices <<< "$selection"
    declare -A seen=()
    valid=true
    selected=()
    for choice in "${choices[@]}"; do
        if [[ ! "$choice" =~ ^[1-5]$ ]]; then
            echo "Invalid selection '$choice'; use numbers 1-5, a comma-separated list, or 6 for all." >&2
            valid=false
            break
        fi
        if [[ -n "${seen[$choice]:-}" ]]; then
            echo "Experiment $choice was selected more than once." >&2
            valid=false
            break
        fi
        seen[$choice]=1
        selected+=("${labels[$((choice - 1))]}")
    done
    if [[ "$valid" == true ]]; then
        break
    fi
done

EXPERIMENTS_SELECTED="$(IFS=,; printf '%s' "${selected[*]}")"
export EXPERIMENTS_SELECTED
echo "Execution mode: $mode"
echo "Selected experiments (in order): ${selected[*]}"

if [[ "$mode" == evaluate ]]; then
    evaluation_status=0
    for experiment in "${selected[@]}"; do
        module_dir="$MODULE_ROOT/$experiment"
        report_script="$module_dir/report.py"
        module_output_root="$EXPERIMENTS_OUTPUT_ROOT/$experiment"
        if [[ ! -f "$report_script" ]]; then
            echo "Error: report script not found for '$experiment': $report_script" >&2
            evaluation_status=1
            continue
        fi

        shopt -s nullglob
        result_dirs=("$module_output_root"/*)
        shopt -u nullglob
        found_result=false
        evaluation_index=0
        for output_dir in "${result_dirs[@]}"; do
            [[ -d "$output_dir" ]] || continue
            found_result=true
            missing_artifacts=()
            case "$experiment" in
                runtime|empty-block|startup) required_artifacts=(metadata.json) ;;
                compliance) required_artifacts=(metadata.json results.jsonl) ;;
                size) required_artifacts=(source_size.tsv binary_size.tsv) ;;
            esac
            for artifact in "${required_artifacts[@]}"; do
                if [[ ! -f "$output_dir/$artifact" ]]; then
                    missing_artifacts+=("$artifact")
                fi
            done
            if (( ${#missing_artifacts[@]} > 0 )); then
                echo "Error: skipping incomplete $experiment results in '$output_dir'; missing: ${missing_artifacts[*]}." >&2
                evaluation_status=1
                continue
            fi

            echo "=== Re-evaluating $experiment results: $(basename "$output_dir") ==="
            evaluation_index=$((evaluation_index + 1))
            if run_logged "$RUN_LOG_DIR/$experiment/evaluation-$evaluation_index.log" \
                "Evaluate $experiment saved run $(basename "$output_dir")" \
                python3 "$report_script" --output-dir "$output_dir"; then
                echo "=== Re-evaluated $experiment results: $output_dir ==="
            else
                status=$?
                echo "Error: could not evaluate $experiment results in '$output_dir' (exit $status)." >&2
                evaluation_status=1
            fi
        done
        if [[ "$found_result" != true ]]; then
            echo "Error: no saved $experiment results found in '$module_output_root'." >&2
            evaluation_status=1
        fi
    done
    if (( evaluation_status != 0 )); then
        exit "$evaluation_status"
    fi
    echo "All selected saved results were evaluated successfully."
    exit 0
fi

for experiment in "${selected[@]}"; do
    module_script="$MODULE_ROOT/$experiment/run.sh"
    if [[ ! -f "$module_script" ]]; then
        echo "Error: entry point not found for '$experiment': $module_script" >&2
        exit 1
    fi
    if configure_module "$experiment"; then
        :
    else
        status=$?
        exit "$status"
    fi
done
fi

if [[ ! -f "$SETUP_SCRIPT" ]]; then
    echo "Error: setup script not found: $SETUP_SCRIPT" >&2
    exit 1
fi

if [[ -z "$RESUME_SESSION" ]]; then
    export EXPERIMENT_RUN_ID="${EXPERIMENT_RUN_ID:-$(date -u +%Y%m%dT%H%M%SZ)-$$}"
    BASE_EXPERIMENT_RUN_ID="$EXPERIMENT_RUN_ID"
    export BASE_EXPERIMENT_RUN_ID
    if [[ "$DETACH_MODE" == true ]]; then
        STATE_FILE="$RUN_LOG_DIR/runner-state.sh"
        (
            umask 077
            {
                declare -p selected runtime_args empty_block_args startup_args compliance_args size_args runtime_workloads empty_block_clients
                printf 'runtime_iterations=%q\n' "$runtime_iterations"
                printf 'mode=%q\n' "$mode"
                printf 'EXPERIMENTS_SELECTED=%q\n' "$EXPERIMENTS_SELECTED"
                printf 'EXPERIMENT_RUN_ID=%q\n' "$EXPERIMENT_RUN_ID"
                printf 'BASE_EXPERIMENT_RUN_ID=%q\n' "$BASE_EXPERIMENT_RUN_ID"
            } > "$STATE_FILE"
        )
        if [[ -v EB_PRIVATE_KEY ]]; then export EB_PRIVATE_KEY; fi
        nohup setsid bash "$SCRIPT_DIR/run.sh" --resume "$LOG_SESSION_ID" \
            </dev/null >> "$RUN_LOG_DIR/runner.log" 2>&1 &
        RUNNER_PID=$!
        printf '%s\n' "$RUNNER_PID" > "$RUN_LOG_DIR/runner.pid"
        echo "Detached experiment session: $LOG_SESSION_ID (PID $RUNNER_PID)."
        echo "Reattach: bash experiments/run.sh --attach $LOG_SESSION_ID"
        echo "Stop: bash experiments/run.sh --stop $LOG_SESSION_ID"
        exit 0
    fi
fi

if [[ -v EB_PRIVATE_KEY ]]; then export -n EB_PRIVATE_KEY; fi
echo "Run ID: $EXPERIMENT_RUN_ID"
echo "Output root: $EXPERIMENTS_OUTPUT_ROOT"
echo "Running shared setup once..."
if run_logged "$RUN_LOG_DIR/setup.log" "Shared setup (single invocation)" bash "$SETUP_SCRIPT"; then
    :
else
    status=$?
    echo "Error: shared setup failed (exit $status); no experiments were started." >&2
    exit "$status"
fi

for experiment in "${selected[@]}"; do
    module_script="$MODULE_ROOT/$experiment/run.sh"
    echo "=== Starting $experiment experiment ==="
    args_name="${experiment//-/_}_args"
    declare -n module_args="$args_name"
    iterations=1
    workload_count=1
    client_count=1
    if [[ "$experiment" == runtime ]]; then iterations="$runtime_iterations"; fi
    if [[ "$experiment" == runtime ]]; then workload_count="${#runtime_workloads[@]}"; fi
    if [[ "$experiment" == empty-block ]]; then client_count="${#empty_block_clients[@]}"; fi
    for ((iteration = 1; iteration <= iterations; iteration++)); do
        for ((workload_index = 1; workload_index <= workload_count; workload_index++)); do
            for ((client_index = 1; client_index <= client_count; client_index++)); do
                export EXPERIMENT_RUN_ID="$BASE_EXPERIMENT_RUN_ID"
                if [[ "$experiment" == runtime && ( "$iterations" -gt 1 || "$workload_count" -gt 1 ) ]]; then
                    EXPERIMENT_RUN_ID="${BASE_EXPERIMENT_RUN_ID}-runtime-${iteration}-${workload_index}"
                    if (( ${#EXPERIMENT_RUN_ID} > 128 )); then
                        echo "Error: runtime workload/iteration run ID exceeds the 128-character limit." >&2
                        exit 2
                    fi
                    export EXPERIMENT_RUN_ID
                fi
                if [[ "$experiment" == empty-block ]]; then
                    empty_block_client="${empty_block_clients[$((client_index - 1))]}"
                    EXPERIMENT_RUN_ID="${BASE_EXPERIMENT_RUN_ID}-empty-block-${empty_block_client}"
                    if (( ${#EXPERIMENT_RUN_ID} > 128 )); then
                        echo "Error: empty-block client run ID exceeds the 128-character limit." >&2
                        exit 2
                    fi
                    export EXPERIMENT_RUN_ID
                    echo "--- Empty-block client $client_index of $client_count: $empty_block_client ---"
                    module_command=(bash "$module_script" --client "$empty_block_client" "${module_args[@]}")
                fi
                if [[ "$experiment" == runtime ]]; then
                    echo "--- Workload $workload_index of $workload_count: ${runtime_workloads[$((workload_index - 1))]} (iteration $iteration of $iterations) ---"
                    module_command=(bash "$module_script" "${module_args[@]}" --workload-type "${runtime_workloads[$((workload_index - 1))]}")
                elif [[ "$experiment" != empty-block ]]; then
                    module_command=(bash "$module_script" "${module_args[@]}")
                fi
                if (( iterations > 1 )) && [[ "$experiment" != runtime ]]; then echo "--- Iteration $iteration of $iterations ---"; fi
                if [[ "$experiment" == empty-block && -v EB_PRIVATE_KEY ]]; then export EB_PRIVATE_KEY; fi
                module_log_file="$RUN_LOG_DIR/$experiment/run-${EXPERIMENT_RUN_ID}.log"
                if run_logged "$module_log_file" "Run $experiment experiment (run ID $EXPERIMENT_RUN_ID)" \
                    "${module_command[@]}"; then
                    :
                else
                    status=$?
                    echo "Error: $experiment experiment failed; later experiments were not started." >&2
                    exit "$status"
                fi
                if [[ "$experiment" == empty-block && -v EB_PRIVATE_KEY ]]; then export -n EB_PRIVATE_KEY; fi
            done
        done
    done
    unset -n module_args
    echo "=== Finished $experiment experiment ==="
done

export EXPERIMENT_RUN_ID="$BASE_EXPERIMENT_RUN_ID"
echo "All selected experiments completed successfully."