#!/bin/bash


set -e


NODE_NAME="VPS-Node-$(hostname)"
P2P_PORT=4001
ENABLE_DHT=true
ENABLE_BOOTSTRAP=true
ENABLE_RELAY=false


RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' 

print_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

print_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

print_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}


check_port() {
    local port=$1
    if netstat -tuln | grep -q ":$port "; then
        print_warning "Port $port is already in use"
        return 1
    fi
    return 0
}


check_docker() {
    if ! command -v docker &> /dev/null; then
        print_error "Docker is not installed. Please install Docker first."
        echo "Installation guide: https://docs.docker.com/engine/install/"
        exit 1
    fi
    
    if ! command -v docker-compose &> /dev/null; then
        print_error "Docker Compose is not installed. Please install Docker Compose first."
        echo "Installation guide: https://docs.docker.com/compose/install/"
        exit 1
    fi
}


setup_firewall() {
    local port=$1
    
    print_info "Setting up firewall rules for port $port"
    
    # Check if ufw is available
    if command -v ufw &> /dev/null; then
        print_info "Using UFW to open port $port"
        sudo ufw allow $port/tcp
        sudo ufw --force enable
        print_success "UFW rule added for port $port"
    # Check if firewall-cmd is available (CentOS/RHEL)
    elif command -v firewall-cmd &> /dev/null; then
        print_info "Using firewall-cmd to open port $port"
        sudo firewall-cmd --permanent --add-port=$port/tcp
        sudo firewall-cmd --reload
        print_success "Firewall rule added for port $port"
    # Check if iptables is available
    elif command -v iptables &> /dev/null; then
        print_info "Using iptables to open port $port"
        sudo iptables -A INPUT -p tcp --dport $port -j ACCEPT
        # Try to save iptables rules
        if command -v iptables-save &> /dev/null; then
            sudo iptables-save > /etc/iptables/rules.v4 2>/dev/null || true
        fi
        print_success "Iptables rule added for port $port"
    else
        print_warning "No firewall management tool found. Please manually open port $port"
        print_warning "You may need to configure your VPS provider's firewall as well"
    fi
}

get_public_ip() {
    local ip
    ip=$(curl -s ifconfig.me || curl -s ipinfo.io/ip || curl -s icanhazip.com)
    if [[ -n "$ip" ]]; then
        echo "$ip"
    else
        print_warning "Could not determine public IP"
        echo "unknown"
    fi
}

deploy() {
    print_info "Starting P2P node deployment..."
    
    check_docker
    
    print_info "Building Docker image..."
    docker-compose build
    
    print_info "Stopping existing containers..."
    docker-compose down 2>/dev/null || true
    
    setup_firewall $P2P_PORT
    
    print_info "Starting P2P node container..."
    docker-compose up -d
    
    sleep 5
    
    if docker-compose ps | grep -q "Up"; then
        local public_ip=$(get_public_ip)
        print_success "P2P node deployed successfully!"
        print_info "Container status:"
        docker-compose ps
        echo ""
        print_info "Node details:"
        echo "  - Node name: $NODE_NAME"
        echo "  - P2P port: $P2P_PORT"
        echo "  - Public IP: $public_ip"
        echo "  - Multiaddr: /ip4/$public_ip/tcp/$P2P_PORT/p2p/[PEER_ID]"
        echo ""
        print_info "To connect to this node from another peer, use:"
        echo "  ./node_eeb --connect /ip4/$public_ip/tcp/$P2P_PORT/p2p/[PEER_ID]"
        echo ""
        print_info "To view logs:"
        echo "  docker-compose logs -f"
        echo ""
        print_info "To stop the node:"
        echo "  docker-compose down"
    else
        print_error "Failed to start P2P node container"
        print_info "Checking logs..."
        docker-compose logs
        exit 1
    fi
}

show_usage() {
    echo "Usage: $0 [OPTIONS]"
    echo ""
    echo "Options:"
    echo "  --name NAME         Set node name (default: VPS-Node-\$(hostname))"
    echo "  --port PORT         Set P2P port (default: 4001)"
    echo "  --no-dht           Disable DHT"
    echo "  --no-bootstrap     Disable bootstrap nodes"
    echo "  --relay            Enable relay mode"
    echo "  --logs             Show container logs"
    echo "  --stop             Stop the P2P node"
    echo "  --restart          Restart the P2P node"
    echo "  --status           Show node status"
    echo "  --help             Show this help message"
    echo ""
    echo "Examples:"
    echo "  $0                                    # Deploy with default settings"
    echo "  $0 --name MyNode --port 8080         # Deploy with custom name and port"
    echo "  $0 --logs                            # Show logs"
    echo "  $0 --stop                            # Stop the node"
}

while [[ $# -gt 0 ]]; do
    case $1 in
        --name)
            NODE_NAME="$2"
            shift 2
            ;;
        --port)
            P2P_PORT="$2"
            shift 2
            ;;
        --no-dht)
            ENABLE_DHT=false
            shift
            ;;
        --no-bootstrap)
            ENABLE_BOOTSTRAP=false
            shift
            ;;
        --relay)
            ENABLE_RELAY=true
            shift
            ;;
        --logs)
            docker-compose logs -f
            exit 0
            ;;
        --stop)
            print_info "Stopping P2P node..."
            docker-compose down
            print_success "P2P node stopped"
            exit 0
            ;;
        --restart)
            print_info "Restarting P2P node..."
            docker-compose down
            docker-compose up -d
            print_success "P2P node restarted"
            exit 0
            ;;
        --status)
            print_info "P2P node status:"
            docker-compose ps
            exit 0
            ;;
        --help)
            show_usage
            exit 0
            ;;
        *)
            print_error "Unknown option: $1"
            show_usage
            exit 1
            ;;
    esac
done

export NODE_NAME P2P_PORT ENABLE_DHT ENABLE_BOOTSTRAP ENABLE_RELAY

deploy