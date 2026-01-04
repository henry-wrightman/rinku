import * as os from 'os';
import * as fsp from 'fs/promises';
import * as path from 'path';
import client from 'prom-client';

const promRegister = new client.Registry();
client.collectDefaultMetrics({ register: promRegister });

export const dagNodesGauge = new client.Gauge({
  name: 'rinku_dag_nodes_total',
  help: 'Total number of nodes in the DAG',
  registers: [promRegister],
});

export const dagTipsGauge = new client.Gauge({
  name: 'rinku_dag_tips_total',
  help: 'Current number of tips in the DAG',
  registers: [promRegister],
});

export const accountsGauge = new client.Gauge({
  name: 'rinku_accounts_total',
  help: 'Total number of accounts',
  registers: [promRegister],
});

export const mempoolSizeGauge = new client.Gauge({
  name: 'rinku_mempool_size',
  help: 'Current mempool size',
  registers: [promRegister],
});

export const checkpointHeightGauge = new client.Gauge({
  name: 'rinku_checkpoint_height',
  help: 'Current checkpoint height',
  registers: [promRegister],
});

export const checkpointLatencyHistogram = new client.Histogram({
  name: 'rinku_checkpoint_latency_seconds',
  help: 'Time to create checkpoints',
  buckets: [0.1, 0.5, 1, 2, 5, 10, 30],
  registers: [promRegister],
});

export const txProcessedCounter = new client.Counter({
  name: 'rinku_transactions_processed_total',
  help: 'Total number of transactions processed',
  registers: [promRegister],
});

export const txSubmittedCounter = new client.Counter({
  name: 'rinku_transactions_submitted_total',
  help: 'Total number of transactions submitted via API',
  registers: [promRegister],
});

export const txRejectedCounter = new client.Counter({
  name: 'rinku_transactions_rejected_total',
  help: 'Total number of transactions rejected',
  labelNames: ['reason'],
  registers: [promRegister],
});

export const gasPriceGauge = new client.Gauge({
  name: 'rinku_gas_price_current',
  help: 'Current gas price in RKU',
  registers: [promRegister],
});

export const peerCountGauge = new client.Gauge({
  name: 'rinku_peers_connected',
  help: 'Number of connected peers',
  registers: [promRegister],
});

export const gossipMessagesCounter = new client.Counter({
  name: 'rinku_gossip_messages_total',
  help: 'Total gossip messages sent/received',
  labelNames: ['direction', 'type'],
  registers: [promRegister],
});

export const validatorCountGauge = new client.Gauge({
  name: 'rinku_validators_active',
  help: 'Number of active validators',
  registers: [promRegister],
});

export const totalStakeGauge = new client.Gauge({
  name: 'rinku_stake_total',
  help: 'Total staked RKU',
  registers: [promRegister],
});

export const totalSupplyGauge = new client.Gauge({
  name: 'rinku_supply_total',
  help: 'Total RKU supply',
  registers: [promRegister],
});

export const emissionGauge = new client.Gauge({
  name: 'rinku_emission_per_checkpoint',
  help: 'Current emission rate per checkpoint',
  registers: [promRegister],
});

export const tipConsolidationCounter = new client.Counter({
  name: 'rinku_tip_consolidations_total',
  help: 'Total number of tip consolidation transactions',
  registers: [promRegister],
});

export const tipsConsolidatedCounter = new client.Counter({
  name: 'rinku_tips_consolidated_total',
  help: 'Total number of tips consolidated',
  registers: [promRegister],
});

export const forksDetectedCounter = new client.Counter({
  name: 'rinku_forks_detected_total',
  help: 'Total number of forks detected',
  registers: [promRegister],
});

export const forksResolvedCounter = new client.Counter({
  name: 'rinku_forks_resolved_total',
  help: 'Total number of forks resolved',
  registers: [promRegister],
});

export const slashingEventsCounter = new client.Counter({
  name: 'rinku_slashing_events_total',
  help: 'Total slashing events',
  labelNames: ['type'],
  registers: [promRegister],
});

export const zkProofsGeneratedCounter = new client.Counter({
  name: 'rinku_zk_proofs_generated_total',
  help: 'Total ZK proofs generated',
  registers: [promRegister],
});

export const zkProofsVerifiedCounter = new client.Counter({
  name: 'rinku_zk_proofs_verified_total',
  help: 'Total ZK proofs verified',
  labelNames: ['result'],
  registers: [promRegister],
});

export async function getPrometheusMetrics(): Promise<string> {
  return promRegister.metrics();
}

export function getPrometheusContentType(): string {
  return promRegister.contentType;
}

export interface SystemTelemetry {
  cpu: {
    usage: number;
    model: string;
    cores: number;
  };
  memory: {
    heapUsed: number;
    heapTotal: number;
    rss: number;
    systemTotal: number;
    systemFree: number;
  };
  network: {
    bytesIn: number;
    bytesOut: number;
    rateIn: number;
    rateOut: number;
  };
  disk: {
    dataDir: number;
  };
  uptime: number;
  processUptime: number;
}

export class TelemetryService {
  private lastCpuInfo: { idle: number; total: number } | null = null;
  private lastNetworkSample: { bytesIn: number; bytesOut: number; time: number } | null = null;
  private totalBytesIn = 0;
  private totalBytesOut = 0;
  private startTime = Date.now();
  private dataDir: string;
  private cachedDiskSize = 0;
  private diskScanInProgress = false;
  private diskScanInterval: NodeJS.Timeout | null = null;
  private static readonly DISK_SCAN_INTERVAL_MS = 30000;

  constructor(dataDir: string = '.rinku-data') {
    this.dataDir = dataDir;
    this.scheduleDiskScan();
  }

  private scheduleDiskScan(): void {
    this.scanDiskAsync();
    this.diskScanInterval = setInterval(() => {
      this.scanDiskAsync();
    }, TelemetryService.DISK_SCAN_INTERVAL_MS);
  }

  private async scanDiskAsync(): Promise<void> {
    if (this.diskScanInProgress) return;
    this.diskScanInProgress = true;
    try {
      this.cachedDiskSize = await this.getDirectorySizeAsync(this.dataDir);
    } catch {
      // Keep cached value on error
    } finally {
      this.diskScanInProgress = false;
    }
  }

  stop(): void {
    if (this.diskScanInterval) {
      clearInterval(this.diskScanInterval);
      this.diskScanInterval = null;
    }
  }

  recordNetworkIn(bytes: number): void {
    this.totalBytesIn += bytes;
  }

  recordNetworkOut(bytes: number): void {
    this.totalBytesOut += bytes;
  }

  private getCpuUsage(): number {
    const cpus = os.cpus();
    let idle = 0;
    let total = 0;

    for (const cpu of cpus) {
      idle += cpu.times.idle;
      total += cpu.times.user + cpu.times.nice + cpu.times.sys + cpu.times.idle + cpu.times.irq;
    }

    if (this.lastCpuInfo) {
      const idleDiff = idle - this.lastCpuInfo.idle;
      const totalDiff = total - this.lastCpuInfo.total;
      const usage = totalDiff > 0 ? ((totalDiff - idleDiff) / totalDiff) * 100 : 0;
      this.lastCpuInfo = { idle, total };
      return Math.round(usage * 10) / 10;
    }

    this.lastCpuInfo = { idle, total };
    return 0;
  }

  private async getDirectorySizeAsync(dirPath: string): Promise<number> {
    try {
      const stat = await fsp.stat(dirPath).catch(() => null);
      if (!stat || !stat.isDirectory()) return 0;
      
      let totalSize = 0;
      const files = await fsp.readdir(dirPath);
      
      for (const file of files) {
        const filePath = path.join(dirPath, file);
        const stats = await fsp.stat(filePath).catch(() => null);
        if (!stats) continue;
        
        if (stats.isDirectory()) {
          totalSize += await this.getDirectorySizeAsync(filePath);
        } else {
          totalSize += stats.size;
        }
      }
      
      return totalSize;
    } catch {
      return 0;
    }
  }

  private getNetworkRates(): { rateIn: number; rateOut: number } {
    const now = Date.now();
    
    if (this.lastNetworkSample) {
      const timeDiff = (now - this.lastNetworkSample.time) / 1000;
      const bytesInDiff = this.totalBytesIn - this.lastNetworkSample.bytesIn;
      const bytesOutDiff = this.totalBytesOut - this.lastNetworkSample.bytesOut;
      
      this.lastNetworkSample = { bytesIn: this.totalBytesIn, bytesOut: this.totalBytesOut, time: now };
      
      return {
        rateIn: timeDiff > 0 ? bytesInDiff / timeDiff : 0,
        rateOut: timeDiff > 0 ? bytesOutDiff / timeDiff : 0
      };
    }
    
    this.lastNetworkSample = { bytesIn: this.totalBytesIn, bytesOut: this.totalBytesOut, time: now };
    return { rateIn: 0, rateOut: 0 };
  }

  collect(): SystemTelemetry {
    const memUsage = process.memoryUsage();
    const cpus = os.cpus();
    const networkRates = this.getNetworkRates();

    return {
      cpu: {
        usage: this.getCpuUsage(),
        model: cpus[0]?.model || 'Unknown',
        cores: cpus.length
      },
      memory: {
        heapUsed: memUsage.heapUsed,
        heapTotal: memUsage.heapTotal,
        rss: memUsage.rss,
        systemTotal: os.totalmem(),
        systemFree: os.freemem()
      },
      network: {
        bytesIn: this.totalBytesIn,
        bytesOut: this.totalBytesOut,
        rateIn: networkRates.rateIn,
        rateOut: networkRates.rateOut
      },
      disk: {
        dataDir: this.cachedDiskSize
      },
      uptime: os.uptime(),
      processUptime: (Date.now() - this.startTime) / 1000
    };
  }

  getSpecs(): { cpu: string; cores: number; totalRam: number; platform: string; arch: string } {
    const cpus = os.cpus();
    return {
      cpu: cpus[0]?.model || 'Unknown',
      cores: cpus.length,
      totalRam: os.totalmem(),
      platform: os.platform(),
      arch: os.arch()
    };
  }
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

export function formatDuration(seconds: number): string {
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const secs = Math.floor(seconds % 60);

  if (days > 0) return `${days}d ${hours}h ${minutes}m`;
  if (hours > 0) return `${hours}h ${minutes}m ${secs}s`;
  if (minutes > 0) return `${minutes}m ${secs}s`;
  return `${secs}s`;
}
