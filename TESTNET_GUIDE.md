# Rinku Testnet Guide: 3-Node Setup

This guide walks you through setting up a moderately aggressive testnet with 3 nodes: Umbrel, Laptop, and Replit.

## Network Topology

```
┌─────────────────────────────────────────────────────────────────┐
│                        INTERNET                                  │
└─────────────────────────────────────────────────────────────────┘
          │                                    │
          │                                    │
    ┌─────▼─────┐                       ┌─────▼─────┐
    │  REPLIT   │◄─────────────────────►│   HOME    │
    │   NODE    │     (public peer)     │  ROUTER   │
    │ (port 80) │                       └─────┬─────┘
    └───────────┘                             │
                                    ┌─────────┴─────────┐
                                    │                   │
                              ┌─────▼─────┐       ┌─────▼─────┐
                              │  UMBREL   │◄─────►│  LAPTOP   │
                              │   NODE    │ (LAN) │   NODE    │
                              │ (port 3000)│       │(port 3001)│
                              └───────────┘       └───────────┘
```

## Prerequisites

1. **Umbrel**: Node.js 20+ installed (or run via Docker)
2. **Laptop**: Node.js 20+ and npm
3. **Replit**: Already configured with this project

---

## Step 1: Get IP Addresses

### Find your local IPs

**On Umbrel:**
```bash
hostname -I | awk '{print $1}'
# Example: 192.168.1.100
```

**On Laptop:**
```bash
# macOS
ipconfig getifaddr en0
# Linux
hostname -I | awk '{print $1}'
# Example: 192.168.1.101
```

### Get Replit URL
Your Replit node URL will be like: `https://your-project-name.your-username.repl.co`

---

## Step 2: Configure Each Node

### Node 1: Umbrel (port 3000)

SSH into your Umbrel and run:

```bash
# Clone or copy the project
cd /path/to/rinku

# Set environment variables
export NODE_ID="umbrel"
export SELF_URL="http://192.168.1.100:3000"
export NODE_PEERS="http://192.168.1.101:3001,https://your-project.your-username.repl.co"
export PORT=3000
export GOSSIP_ENABLED=true
export GOSSIP_INTERVAL_MS=200
export CRYPTO_WORKERS=2

# Start the node
npm run dev -w @rinku/node
```

### Node 2: Laptop (port 3001)

In a terminal:

```bash
cd /path/to/rinku

# Set environment variables
export NODE_ID="laptop"
export SELF_URL="http://192.168.1.101:3001"
export NODE_PEERS="http://192.168.1.100:3000,https://your-project.your-username.repl.co"
export PORT=3001
export GOSSIP_ENABLED=true
export GOSSIP_INTERVAL_MS=200
export CRYPTO_WORKERS=4

# Start the node
npm run dev -w @rinku/node
```

### Node 3: Replit (port 5000)

In Replit, set these environment variables via the Secrets tab:

| Key | Value |
|-----|-------|
| `NODE_ID` | `replit` |
| `SELF_URL` | `https://your-project.your-username.repl.co` |
| `NODE_PEERS` | `http://YOUR_PUBLIC_IP:3000,http://YOUR_PUBLIC_IP:3001` |
| `GOSSIP_ENABLED` | `true` |
| `GOSSIP_INTERVAL_MS` | `200` |

**Important**: The Replit node needs to reach your home network. Options:
- **Port forwarding**: Forward ports 3000 and 3001 on your router to the Umbrel and Laptop respectively
- **Tailscale/ZeroTier**: Use a mesh VPN for easier connectivity
- **One-way gossip**: Even if Replit can't reach home nodes, home nodes can push to Replit

---

## Step 3: Verify Connectivity

### Check each node is running

```bash
# From laptop, check all nodes
curl http://192.168.1.100:3000/api/status  # Umbrel
curl http://localhost:3001/api/status       # Laptop (local)
curl https://your-project.your-username.repl.co/api/status  # Replit
```

Expected response:
```json
{
  "tipCount": 0,
  "checkpointHeight": 0,
  "validatorCount": 0,
  "peerCount": 2
}
```

### Check peer discovery

After 30 seconds, each node should show `peerCount: 2`.

---

## Step 4: Start the Metrics Dashboard

From your laptop (or any machine with connectivity to all nodes):

```bash
cd /path/to/rinku

TESTNET_NODES="umbrel=http://192.168.1.100:3000,laptop=http://localhost:3001,replit=https://your-project.your-username.repl.co" \
  npm run testnet-metrics -w @rinku/node
```

You'll see a live dashboard:
```
╔══════════════════════════════════════════════════════════════════╗
║              RINKU TESTNET METRICS DASHBOARD                     ║
╠══════════════════════════════════════════════════════════════════╣
║  Uptime: 00:05:32                                                ║
╠══════════════════════════════════════════════════════════════════╣
║  NODE STATUS                                                      ║
╠══════════════════════════════════════════════════════════════════╣
║  🟢 umbrel       | Tips:    142 | CP:    1 | Peers: 2 | 45ms     ║
║     TPS:   2.40 curr |   2.15 avg |   4.80 peak                  ║
║  🟢 laptop       | Tips:    142 | CP:    1 | Peers: 2 | 12ms     ║
║     TPS:   2.40 curr |   2.18 avg |   4.90 peak                  ║
║  🟢 replit       | Tips:    140 | CP:    1 | Peers: 2 | 180ms    ║
║     TPS:   2.35 curr |   2.10 avg |   4.60 peak                  ║
╠══════════════════════════════════════════════════════════════════╣
║  CONSENSUS HEALTH                                                 ║
╠══════════════════════════════════════════════════════════════════╣
║  Sync Status: 🟢 ALL SYNCED          Height Drift:    2 tips     ║
║  Checkpoint:  🟢 AGREED                                          ║
╚══════════════════════════════════════════════════════════════════╝
```

---

## Step 5: Generate Load (Moderately Aggressive)

### Option A: Activity Bot (Recommended)

Run on your laptop to generate sustained transaction load:

```bash
# Target the laptop node for fastest submission
FAUCET_URL=http://localhost:3001 \
  npm run activity-bot -w @rinku/node
```

The activity bot creates accounts, sends transfers, and interacts with contracts.

### Option B: Stress Test (Aggressive)

For burst testing:

```bash
# 100 transactions as fast as possible
FAUCET_URL=http://localhost:3001 \
  npm run stress-test -w @rinku/node
```

### Option C: Manual Transactions via Faucet

Open the explorer at `http://localhost:5173` (or the Replit webview) and:
1. Create multiple accounts
2. Request faucet funds
3. Send transfers between accounts

---

## Step 6: Test Scenarios

### Scenario 1: Normal Operation (30 min)
- Run activity bot at default rate
- Monitor TPS stabilizes around 2-5 TPS
- Verify all nodes stay in sync (height drift < 5)
- Confirm checkpoints are created every ~100 tips

### Scenario 2: Network Partition (5 min)
- Stop the Replit node (Ctrl+C or pause workflow)
- Continue transactions on local nodes
- Verify local nodes continue operating
- Restart Replit and watch it sync back

### Scenario 3: Burst Load (5 min)
- Run stress test: `npm run stress-test -w @rinku/node`
- Monitor if nodes handle 50-100 tx burst
- Check memory usage doesn't spike excessively
- Verify eventual consistency across nodes

### Scenario 4: Validator Staking
- Use explorer to stake on each node
- Verify validator set updates propagate
- Check checkpoint signatures include new validators

---

## Step 7: Collect Results

When done testing, press `Ctrl+C` on the metrics dashboard. It exports:

```bash
# File created: testnet-metrics-1704000000000.json
```

This JSON contains:
- All snapshots with timestamps
- TPS metrics per node
- Consensus health summary
- Duration and total transactions

---

## Troubleshooting

### Nodes not discovering each other

1. Check firewall allows the ports
2. Verify IP addresses are correct
3. Try pinging between machines: `ping 192.168.1.100`

### Replit can't reach home nodes

**Option 1: Port Forwarding**
- Log into your router (usually 192.168.1.1)
- Forward external ports 3000→Umbrel:3000 and 3001→Laptop:3001
- Use your public IP in Replit's `NODE_PEERS`

**Option 2: Use Tailscale**
- Install Tailscale on Umbrel, Laptop, and in Replit (via nix)
- Use Tailscale IPs instead of local IPs

**Option 3: One-way gossip**
- Home nodes push to Replit, Replit doesn't need to reach home
- This works for transaction propagation but limits some sync features

### High latency to Replit

- Expected: 100-300ms latency to Replit is normal
- The metrics dashboard shows 🟡 for 1-3 second latency
- Gossip protocol handles intermittent delays

### Memory issues on Umbrel

If Umbrel has limited RAM:
```bash
export MAX_DAG_NODES=10000  # Limit in-memory transactions
export CRYPTO_WORKERS=1     # Use less parallelism
```

---

## Success Criteria

Your testnet is working well if:

| Metric | Target | Acceptable |
|--------|--------|------------|
| TPS (sustained) | 2-5 TPS | 1-10 TPS |
| Height drift | < 5 tips | < 20 tips |
| Checkpoint consensus | Always agreed | Occasional 1-block delay |
| Node latency | < 500ms | < 3000ms |
| Sync after partition | < 60 seconds | < 5 minutes |

---

## Next Steps After Testnet

1. **Export metrics** and review the JSON for anomalies
2. **Check logs** on each node for errors
3. **Report issues** with specific tip counts and timestamps
4. **Scale up**: Add a 4th node or increase transaction rate
