export interface FinalityMetrics {
  avgTimeToFinality: number;
  medianTimeToFinality: number;
  p95TimeToFinality: number;
  pendingCount: number;
  finalizedCount: number;
  finalityRate: number;
  checkpointLatency: number;
  checkpointsPerMinute: number;
  lastCheckpointAge: number;
  txThroughput: number;
}

interface PendingTx {
  hash: string;
  submittedAt: number;
}

interface FinalityRecord {
  timeToFinality: number;
  timestamp: number;
}

interface CheckpointRecord {
  height: number;
  createdAt: number;
  txCount: number;
}

export class FinalityMetricsService {
  private pendingTxs: Map<string, PendingTx> = new Map();
  private finalityRecords: FinalityRecord[] = [];
  private checkpointRecords: CheckpointRecord[] = [];
  private totalFinalized = 0;
  private lastCheckpointTime = 0;
  private checkpointInterval = 60000;
  
  private readonly MAX_RECORDS = 1000;
  private readonly METRICS_WINDOW_MS = 5 * 60 * 1000;

  setCheckpointInterval(ms: number): void {
    this.checkpointInterval = ms;
  }

  recordTxSubmission(hash: string): void {
    this.pendingTxs.set(hash, {
      hash,
      submittedAt: Date.now()
    });
    
    if (this.pendingTxs.size > this.MAX_RECORDS * 2) {
      const cutoff = Date.now() - this.METRICS_WINDOW_MS * 2;
      for (const [h, tx] of this.pendingTxs) {
        if (tx.submittedAt < cutoff) {
          this.pendingTxs.delete(h);
        }
      }
    }
  }

  recordTxFinalized(hash: string, checkpointTimestamp?: number): void {
    const pending = this.pendingTxs.get(hash);
    if (pending) {
      const finalizedAt = checkpointTimestamp || Date.now();
      const timeToFinality = finalizedAt - pending.submittedAt;
      if (timeToFinality > 0) {
        this.finalityRecords.push({
          timeToFinality,
          timestamp: finalizedAt
        });
        this.totalFinalized++;
        
        if (this.finalityRecords.length > this.MAX_RECORDS) {
          this.finalityRecords = this.finalityRecords.slice(-this.MAX_RECORDS);
        }
      }
      this.pendingTxs.delete(hash);
    }
  }

  pruneStaleEntries(currentHeight: number): number {
    const cutoff = Date.now() - this.METRICS_WINDOW_MS * 2;
    let pruned = 0;
    for (const [hash, tx] of this.pendingTxs) {
      if (tx.submittedAt < cutoff) {
        this.pendingTxs.delete(hash);
        pruned++;
      }
    }
    return pruned;
  }

  recordCheckpoint(height: number, txCount: number, checkpointTimestamp?: number): void {
    const ts = checkpointTimestamp || Date.now();
    this.checkpointRecords.push({
      height,
      createdAt: ts,
      txCount
    });
    this.lastCheckpointTime = ts;
    
    if (this.checkpointRecords.length > 100) {
      this.checkpointRecords = this.checkpointRecords.slice(-100);
    }
  }

  getMetrics(): FinalityMetrics {
    const now = Date.now();
    const windowStart = now - this.METRICS_WINDOW_MS;
    
    const recentRecords = this.finalityRecords.filter(r => r.timestamp > windowStart);
    const times = recentRecords.map(r => r.timeToFinality).sort((a, b) => a - b);
    
    const avgTimeToFinality = times.length > 0
      ? times.reduce((a, b) => a + b, 0) / times.length
      : 0;
    
    const medianTimeToFinality = times.length > 0
      ? times[Math.floor(times.length / 2)]
      : 0;
    
    const p95TimeToFinality = times.length > 0
      ? times[Math.floor(times.length * 0.95)]
      : 0;
    
    const pendingCount = this.pendingTxs.size;
    const finalizedCount = recentRecords.length;
    const totalInWindow = pendingCount + finalizedCount;
    const finalityRate = totalInWindow > 0 ? finalizedCount / totalInWindow : 1;
    
    const recentCheckpoints = this.checkpointRecords.filter(c => c.createdAt > windowStart);
    const checkpointsPerMinute = recentCheckpoints.length / (this.METRICS_WINDOW_MS / 60000);
    
    let checkpointLatency = 0;
    if (recentCheckpoints.length >= 2) {
      const intervals: number[] = [];
      for (let i = 1; i < recentCheckpoints.length; i++) {
        intervals.push(recentCheckpoints[i].createdAt - recentCheckpoints[i - 1].createdAt);
      }
      checkpointLatency = intervals.reduce((a, b) => a + b, 0) / intervals.length;
    }
    
    const lastCheckpointAge = this.lastCheckpointTime > 0 ? now - this.lastCheckpointTime : 0;
    
    const txThroughput = recentCheckpoints.length > 0
      ? recentCheckpoints.reduce((sum, c) => sum + c.txCount, 0) / (this.METRICS_WINDOW_MS / 1000)
      : 0;

    return {
      avgTimeToFinality: Math.round(avgTimeToFinality),
      medianTimeToFinality: Math.round(medianTimeToFinality),
      p95TimeToFinality: Math.round(p95TimeToFinality),
      pendingCount,
      finalizedCount,
      finalityRate: Math.round(finalityRate * 100) / 100,
      checkpointLatency: Math.round(checkpointLatency),
      checkpointsPerMinute: Math.round(checkpointsPerMinute * 100) / 100,
      lastCheckpointAge: Math.round(lastCheckpointAge),
      txThroughput: Math.round(txThroughput * 100) / 100
    };
  }

  toJSON(): { pendingTxs: [string, PendingTx][]; finalityRecords: FinalityRecord[]; checkpointRecords: CheckpointRecord[]; totalFinalized: number; lastCheckpointTime: number } {
    return {
      pendingTxs: Array.from(this.pendingTxs.entries()),
      finalityRecords: this.finalityRecords.slice(-500),
      checkpointRecords: this.checkpointRecords,
      totalFinalized: this.totalFinalized,
      lastCheckpointTime: this.lastCheckpointTime
    };
  }

  static fromJSON(data: any): FinalityMetricsService {
    const service = new FinalityMetricsService();
    if (data.pendingTxs) {
      for (const [hash, tx] of data.pendingTxs) {
        service.pendingTxs.set(hash, tx);
      }
    }
    if (data.finalityRecords) {
      service.finalityRecords = data.finalityRecords;
    }
    if (data.checkpointRecords) {
      service.checkpointRecords = data.checkpointRecords;
    }
    if (data.totalFinalized) {
      service.totalFinalized = data.totalFinalized;
    }
    if (data.lastCheckpointTime) {
      service.lastCheckpointTime = data.lastCheckpointTime;
    }
    return service;
  }
}
