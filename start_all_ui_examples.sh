#!/bin/bash

# Start all UI example servers on different ports

set -e

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$PROJECT_ROOT"

# Load environment variables (ignore comments and invalid lines)
if [ -f .env ]; then
    set -a
    source <(grep -v '^#' .env | grep '=' | sed 's/\r$//')
    set +a
fi

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "🚀 Starting All A2UI Example Servers"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# Stop any existing servers
pkill -f "ui_server|ui_working" 2>/dev/null || true
sleep 2

# Build all examples
echo "📦 Building examples..."
cargo build --example ui_server --example ui_working_support --example ui_working_appointment \
  --example ui_working_events --example ui_working_facilities --example ui_working_inventory

# Start servers
echo ""
echo "🌐 Starting servers..."

PORT=8080 cargo run --example ui_server > /tmp/ui_demo.log 2>&1 &
echo "  ✓ UI Demo (port 8080) - PID: $!"

PORT=8081 cargo run --example ui_working_support > /tmp/ui_support.log 2>&1 &
echo "  ✓ Support Intake (port 8081) - PID: $!"

PORT=8082 cargo run --example ui_working_appointment > /tmp/ui_appointment.log 2>&1 &
echo "  ✓ Appointments (port 8082) - PID: $!"

PORT=8083 cargo run --example ui_working_events > /tmp/ui_events.log 2>&1 &
echo "  ✓ Events (port 8083) - PID: $!"

PORT=8084 cargo run --example ui_working_facilities > /tmp/ui_facilities.log 2>&1 &
echo "  ✓ Facilities (port 8084) - PID: $!"

PORT=8085 cargo run --example ui_working_inventory > /tmp/ui_inventory.log 2>&1 &
echo "  ✓ Inventory (port 8085) - PID: $!"

sleep 5

# Start React client
echo ""
echo "⚛️  Starting React client..."
cd examples/ui_react_client
npm run dev > /tmp/react_client.log 2>&1 &
echo "  ✓ React Client (port 5173) - PID: $!"

sleep 3

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "✅ All Servers Running!"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "🌐 Open: http://localhost:5173"
echo ""
echo "📊 Available Examples:"
echo "  • UI Demo (8080)"
echo "  • Support Intake (8081)"
echo "  • Appointments (8082)"
echo "  • Events (8083)"
echo "  • Facilities (8084)"
echo "  • Inventory (8085)"
echo ""
echo "📝 Logs:"
echo "  tail -f /tmp/ui_*.log"
echo "  tail -f /tmp/react_client.log"
echo ""
echo "🛑 Stop all: pkill -f 'ui_server|ui_working|vite'"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
