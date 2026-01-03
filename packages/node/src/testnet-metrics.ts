import http from 'http';

interface NodeConfig {
  name: string;
  url: string;
}

interface MetricsSnapshot {
  timestamp: number;
  node: string;
  blockHeight: number;
  tipCount: number;
  pendingTxCount: number;
  checkpointHeight: number;
  validatorCount: number;
  totalStaked: number;
  peerCount: number;
  latencyMs: number;
}

interface FinalityMetrics {
  avgTimeToFinalityMs: number;
  maxTimeToFinalityMs: number;
  checkpointLatencyMs: number;
  pendingTxCount: number;
}

interface TPSMetrics {
  currentTps: number;
  avgTps: number;
  peakTps: number;
  totalTxProcessed: number;
}

interface ConsensusHealth {
  nodesInSync: number;
  totalNodes: number;
  maxHeightDrift: number;
  checkpointConsensus: boolean;
}

class TestnetMetrics {
  private nodes: NodeConfig[];
  private snapshots: MetricsSnapshot[] = [];
  private txCounts: Map<string, { count: number; timestamp: number }[]> = new Map();
  private intervalId: NodeJS.Timeout | null = null;
  private startTime: number = 0;

  constructor(nodes: NodeConfig[]) {
    this.nodes = nodes;
  }

  private async fetchJson(url: string, timeout = 5000): Promise<any> {
    return new Promise((resolve, reject) => {
      const parsedUrl = new URL(url);
      const options = {
        hostname: parsedUrl.hostname,
        port: parsedUrl.port || 80,
        path: parsedUrl.pathname + parsedUrl.search,
        method: 'GET',
        timeout,
      };

      const req = http.request(options, (res) => {
        let data = '';
        res.on('data', (chunk) => (data += chunk));
        res.on('end', () => {
          try {
            resolve(JSON.parse(data));
          } catch {
            reject(new Error(`Invalid JSON from ${url}`));
          }
        });
      });

      req.on('error', reject);
      req.on('timeout', () => {
        req.destroy();
        reject(new Error(`Timeout fetching ${url}`));
      });

      req.end();
    });
  }

  private async collectNodeMetrics(node: NodeConfig): Promise<MetricsSnapshot | null> {
    const startTime = Date.now();
    try {
      const [status, finality] = await Promise.all([
        this.fetchJson(`${node.url}/api/status`),
        this.fetchJson(`${node.url}/api/finality`).catch(() => null),
      ]);

      const latencyMs = Date.now() - startTime;

      return {
        timestamp: Date.now(),
        node: node.name,
        blockHeight: status.tipCount || 0,
        tipCount: status.tipCount || 0,
        pendingTxCount: finality?.pendingTransactions || 0,
        checkpointHeight: status.checkpointHeight || 0,
        validatorCount: status.validatorCount || 0,
        totalStaked: status.totalStaked || 0,
        peerCount: status.peerCount || 0,
        latencyMs,
      };
    } catch (err) {
      console.error(`[${node.name}] Error collecting metrics:`, (err as Error).message);
      return null;
    }
  }

  async collectAllMetrics(): Promise<MetricsSnapshot[]> {
    const results = await Promise.all(this.nodes.map((n) => this.collectNodeMetrics(n)));
    const validResults = results.filter((r): r is MetricsSnapshot => r !== null);

    for (const snapshot of validResults) {
      this.snapshots.push(snapshot);
      const nodeHistory = this.txCounts.get(snapshot.node) || [];
      nodeHistory.push({ count: snapshot.tipCount, timestamp: snapshot.timestamp });
      if (nodeHistory.length > 60) nodeHistory.shift();
      this.txCounts.set(snapshot.node, nodeHistory);
    }

    return validResults;
  }

  calculateTPS(nodeName: string): TPSMetrics {
    const history = this.txCounts.get(nodeName) || [];
    if (history.length < 2) {
      return { currentTps: 0, avgTps: 0, peakTps: 0, totalTxProcessed: 0 };
    }

    const intervals: number[] = [];
    for (let i = 1; i < history.length; i++) {
      const txDiff = history[i].count - history[i - 1].count;
      const timeDiff = (history[i].timestamp - history[i - 1].timestamp) / 1000;
      if (timeDiff > 0) {
        intervals.push(txDiff / timeDiff);
      }
    }

    const currentTps = intervals.length > 0 ? intervals[intervals.length - 1] : 0;
    const avgTps = intervals.length > 0 ? intervals.reduce((a, b) => a + b, 0) / intervals.length : 0;
    const peakTps = intervals.length > 0 ? Math.max(...intervals) : 0;
    const totalTxProcessed = history.length > 0 ? history[history.length - 1].count : 0;

    return { currentTps, avgTps, peakTps, totalTxProcessed };
  }

  calculateConsensusHealth(): ConsensusHealth {
    const latestByNode = new Map<string, MetricsSnapshot>();
    for (const snapshot of this.snapshots.slice(-this.nodes.length * 2)) {
      latestByNode.set(snapshot.node, snapshot);
    }

    const heights = Array.from(latestByNode.values()).map((s) => s.tipCount);
    const checkpoints = Array.from(latestByNode.values()).map((s) => s.checkpointHeight);

    const maxHeight = Math.max(...heights, 0);
    const minHeight = Math.min(...heights, 0);
    const maxHeightDrift = maxHeight - minHeight;

    const checkpointConsensus = new Set(checkpoints).size <= 1;
    const nodesInSync = heights.filter((h) => maxHeight - h <= 5).length;

    return {
      nodesInSync,
      totalNodes: latestByNode.size,
      maxHeightDrift,
      checkpointConsensus,
    };
  }

  printDashboard(): void {
    console.clear();
    console.log('╔══════════════════════════════════════════════════════════════════╗');
    console.log('║              RINKU TESTNET METRICS DASHBOARD                     ║');
    console.log('╠══════════════════════════════════════════════════════════════════╣');

    const elapsed = Math.floor((Date.now() - this.startTime) / 1000);
    const hours = Math.floor(elapsed / 3600);
    const minutes = Math.floor((elapsed % 3600) / 60);
    const seconds = elapsed % 60;
    console.log(`║  Uptime: ${hours.toString().padStart(2, '0')}:${minutes.toString().padStart(2, '0')}:${seconds.toString().padStart(2, '0')}                                                   ║`);
    console.log('╠══════════════════════════════════════════════════════════════════╣');

    console.log('║  NODE STATUS                                                      ║');
    console.log('╠══════════════════════════════════════════════════════════════════╣');

    const latestByNode = new Map<string, MetricsSnapshot>();
    for (const snapshot of this.snapshots.slice(-this.nodes.length * 5)) {
      latestByNode.set(snapshot.node, snapshot);
    }

    for (const [name, snapshot] of latestByNode) {
      const tps = this.calculateTPS(name);
      const status = snapshot.latencyMs < 1000 ? '🟢' : snapshot.latencyMs < 3000 ? '🟡' : '🔴';
      console.log(`║  ${status} ${name.padEnd(12)} | Tips: ${snapshot.tipCount.toString().padStart(6)} | CP: ${snapshot.checkpointHeight.toString().padStart(4)} | Peers: ${snapshot.peerCount} | ${snapshot.latencyMs}ms ║`);
      console.log(`║     TPS: ${tps.currentTps.toFixed(2).padStart(6)} curr | ${tps.avgTps.toFixed(2).padStart(6)} avg | ${tps.peakTps.toFixed(2).padStart(6)} peak                ║`);
    }

    console.log('╠══════════════════════════════════════════════════════════════════╣');
    console.log('║  CONSENSUS HEALTH                                                 ║');
    console.log('╠══════════════════════════════════════════════════════════════════╣');

    const health = this.calculateConsensusHealth();
    const syncStatus = health.nodesInSync === health.totalNodes ? '🟢 ALL SYNCED' : `🟡 ${health.nodesInSync}/${health.totalNodes} synced`;
    const cpStatus = health.checkpointConsensus ? '🟢 AGREED' : '🔴 DIVERGED';

    console.log(`║  Sync Status: ${syncStatus.padEnd(20)} Height Drift: ${health.maxHeightDrift.toString().padStart(4)} tips     ║`);
    console.log(`║  Checkpoint:  ${cpStatus.padEnd(20)}                                   ║`);

    console.log('╠══════════════════════════════════════════════════════════════════╣');
    console.log('║  Press Ctrl+C to stop monitoring                                  ║');
    console.log('╚══════════════════════════════════════════════════════════════════╝');
  }

  async start(intervalMs = 5000): Promise<void> {
    this.startTime = Date.now();
    console.log('Starting Rinku Testnet Metrics...');
    console.log(`Monitoring ${this.nodes.length} nodes every ${intervalMs / 1000}s\n`);

    await this.collectAllMetrics();
    this.printDashboard();

    this.intervalId = setInterval(async () => {
      await this.collectAllMetrics();
      this.printDashboard();
    }, intervalMs);
  }

  stop(): void {
    if (this.intervalId) {
      clearInterval(this.intervalId);
      this.intervalId = null;
    }
  }

  exportMetrics(): { snapshots: MetricsSnapshot[]; summary: object } {
    const health = this.calculateConsensusHealth();
    const tpsMetrics = new Map<string, TPSMetrics>();
    for (const node of this.nodes) {
      tpsMetrics.set(node.name, this.calculateTPS(node.name));
    }

    return {
      snapshots: this.snapshots,
      summary: {
        duration: Date.now() - this.startTime,
        totalSnapshots: this.snapshots.length,
        consensusHealth: health,
        tpsByNode: Object.fromEntries(tpsMetrics),
      },
    };
  }
}

const nodes: NodeConfig[] = (process.env.TESTNET_NODES || '')
  .split(',')
  .filter(Boolean)
  .map((entry, i) => {
    const [name, url] = entry.includes('=') ? entry.split('=') : [`node${i + 1}`, entry];
    return { name: name.trim(), url: url.trim() };
  });

if (nodes.length === 0) {
  console.log('Usage: TESTNET_NODES="umbrel=http://192.168.1.x:3000,laptop=http://192.168.1.y:3001,replit=https://your-app.replit.app" npx tsx src/testnet-metrics.ts');
  console.log('\nExample for 3-node testnet:');
  console.log('  TESTNET_NODES="umbrel=http://192.168.1.100:3000,laptop=http://localhost:3001,replit=https://rinku.example.repl.co" npx tsx src/testnet-metrics.ts');
  process.exit(1);
}

const metrics = new TestnetMetrics(nodes);

process.on('SIGINT', () => {
  console.log('\n\nStopping metrics collection...');
  metrics.stop();

  const exported = metrics.exportMetrics();
  const filename = `testnet-metrics-${Date.now()}.json`;
  require('fs').writeFileSync(filename, JSON.stringify(exported, null, 2));
  console.log(`Metrics exported to ${filename}`);
  process.exit(0);
});

metrics.start(5000);
