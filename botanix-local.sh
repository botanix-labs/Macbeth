#!/bin/bash
set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' 

if [ -f .env ]; then
    set -a
    source .env
    set +a
fi

OUTPUT_PATH=${OUTPUT_PATH:-"docker-local/configs"}
COMPOSE_FILE="$OUTPUT_PATH/../docker-compose-generated.yml"
INIT_SCRIPT="$OUTPUT_PATH/../init-bitcoin.sh"

log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

log_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

check_env_file() {
    if [ ! -f .env ]; then
        log_warning ".env file not found. Using default configuration."
        log_info "To customize, copy .env.example to .env and edit it:"
        echo "  cp .env.example .env"
        echo ""
    fi
}

cmd_start() {
    log_info "Starting Botanix local environment..."
    echo ""

    check_env_file

    log_info "Step 1/3: Setting up environment and generating configurations..."
    if bash setup-local-env.sh; then
        log_success "Setup completed"
    else
        log_error "Setup failed"
        exit 1
    fi

    echo ""

    log_info "Step 2/3: Starting Docker services..."
    if [ ! -f "$COMPOSE_FILE" ]; then
        log_error "Compose file not found: $COMPOSE_FILE"
        exit 1
    fi

    if docker compose -f "$COMPOSE_FILE" up -d; then
        log_success "Docker services started"
    else
        log_error "Failed to start Docker services"
        exit 1
    fi

    echo ""

    log_info "Waiting for services to be ready..."
    sleep 5

    log_info "Step 3/3: Initializing Bitcoin node..."
    if [ ! -f "$INIT_SCRIPT" ]; then
        log_error "Init script not found: $INIT_SCRIPT"
        exit 1
    fi

    if bash "$INIT_SCRIPT"; then
        log_success "Bitcoin node initialized"
    else
        log_warning "Bitcoin initialization had issues (this might be expected if already initialized)"
    fi

    echo ""
    log_success "Botanix local environment is up and running!"
    echo ""
    echo "Useful commands:"
    echo "  botanix-local services  - View running services"
    echo "  botanix-local logs      - View logs"
    echo "  botanix-local stop      - Stop all services"
}

cmd_stop() {
    log_info "Stopping Botanix local environment..."

    if [ ! -f "$COMPOSE_FILE" ]; then
        log_warning "Compose file not found: $COMPOSE_FILE"
        log_info "Services might not be running or already stopped"
        exit 0
    fi

    if docker compose -f "$COMPOSE_FILE" down; then
        log_success "All services stopped"
    else
        log_error "Failed to stop services"
        exit 1
    fi
}

cmd_restart() {
    log_info "Restarting Botanix local environment..."
    cmd_stop
    echo ""
    cmd_start
}

cmd_clean() {
    log_warning "This will remove all containers, volumes, and generated configurations!"
    read -p "Are you sure? (yes/no): " -r
    echo

    if [[ ! $REPLY =~ ^[Yy][Ee][Ss]$ ]]; then
        log_info "Clean cancelled"
        exit 0
    fi

    log_info "Cleaning up Botanix local environment..."

    if [ -f "$COMPOSE_FILE" ]; then
        log_info "Stopping services..."
        docker compose -f "$COMPOSE_FILE" down -v 2>/dev/null || true
    fi

    if [ -d "$OUTPUT_PATH" ]; then
        log_info "Removing generated files in $OUTPUT_PATH..."
        rm -rf "$OUTPUT_PATH"
    fi

    if [ -f "docker-local/docker-compose-generated.yml" ]; then
        log_info "Removing docker-local/docker-compose-generated.yml..."
        rm -rf "docker-local/docker-compose-generated.yml"
    fi
    if [ -f "docker-local/init-bitcoin.sh" ]; then
        log_info "Removing docker-local/init-bitcoin.sh..."
        rm -rf "docker-local/init-bitcoin.sh"
    fi

    PROJECT_PREFIX=${PROJECT_PREFIX:-"botanix"}
    NETWORK_NAME="${PROJECT_PREFIX}-local"
    if docker network inspect "$NETWORK_NAME" > /dev/null 2>&1; then
        log_info "Removing docker network: $NETWORK_NAME..."
        docker network rm "$NETWORK_NAME" 2>/dev/null || true
    fi

    log_success "Cleanup complete"
}

cmd_services() {
    log_info "Botanix Local Services Status"
    echo ""

    if [ ! -f "$COMPOSE_FILE" ]; then
        log_warning "Compose file not found. Services might not be set up yet."
        echo "Run: botanix-local start"
        exit 0
    fi

    docker compose -f "$COMPOSE_FILE" ps
}

cmd_logs() {
    if [ ! -f "$COMPOSE_FILE" ]; then
        log_error "Compose file not found. Services might not be running."
        exit 1
    fi

    if [ -n "$1" ]; then
        log_info "Showing logs for service: $1"
        docker compose -f "$COMPOSE_FILE" logs -f "$1"
    else
        log_info "Showing logs for all services (Ctrl+C to exit)"
        docker compose -f "$COMPOSE_FILE" logs -f
    fi
}

cmd_exec() {
    if [ -z "$1" ]; then
        log_error "Service name required"
        echo "Usage: botanix-local exec <service> [command]"
        exit 1
    fi

    if [ ! -f "$COMPOSE_FILE" ]; then
        log_error "Compose file not found. Services might not be running."
        exit 1
    fi

    SERVICE=$1
    shift

    if [ $# -eq 0 ]; then
        docker compose -f "$COMPOSE_FILE" exec "$SERVICE" sh
    else
        docker compose -f "$COMPOSE_FILE" exec "$SERVICE" "$@"
    fi
}

cmd_bitcoin() {
    log_info "Executing bitcoin-cli command..."

    docker exec bitcoin-core bitcoin-cli -regtest -rpcuser=foo -rpcpassword=bar "$@"
}

cmd_help() {
    cat << EOF
Botanix Local Development Environment

USAGE:
    botanix-local <command> [options]

COMMANDS:
    start       Setup and start all services (setup → compose up → init bitcoin)
    stop        Stop all running services
    restart     Restart all services (stop → start)
    clean       Remove all containers, volumes, and generated files
    services    Show status of all services
    logs        Show logs for all services (or specific service)
                Usage: botanix-local logs [service-name]
    exec        Execute a command in a running service
                Usage: botanix-local exec <service> [command]
    bitcoin     Execute bitcoin-cli commands
                Usage: botanix-local bitcoin <bitcoin-cli args>
    help        Show this help message

EXAMPLES:
    # First time setup and start
    botanix-local start

    # View running services
    botanix-local services

    # View logs for a specific node
    botanix-local logs poa-1

    # Execute command in a container
    botanix-local exec poa-1 sh

    # Bitcoin CLI commands
    botanix-local bitcoin getblockcount
    botanix-local bitcoin getbalance

    # Stop everything
    botanix-local stop

    # Clean up everything
    botanix-local clean

CONFIGURATION:
    Create a .env file from .env.example to customize settings:
        cp .env.example .env
        vim .env

    Key settings:
        NUM_NODES=3                    # Number of nodes
        MIN_SIGNERS=2                  # Minimum signers
        MAX_SIGNERS=3                  # Maximum signers
        OUTPUT_PATH=docker-local       # Output directory

For more information, see README.md
EOF
}

main() {
    if [ $# -eq 0 ]; then
        cmd_help
        exit 0
    fi

    case "$1" in
        start)
            cmd_start
            ;;
        stop)
            cmd_stop
            ;;
        restart)
            cmd_restart
            ;;
        clean)
            cmd_clean
            ;;
        services|status|ps)
            cmd_services
            ;;
        logs)
            shift
            cmd_logs "$@"
            ;;
        exec)
            shift
            cmd_exec "$@"
            ;;
        bitcoin|btc)
            shift
            cmd_bitcoin "$@"
            ;;
        help|--help|-h)
            cmd_help
            ;;
        *)
            log_error "Unknown command: $1"
            echo ""
            cmd_help
            exit 1
            ;;
    esac
}

main "$@"
