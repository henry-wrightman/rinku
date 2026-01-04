import {
  PROTOCOL_VERSION,
  KNOWN_FEATURES,
  parseVersion,
  compareVersions,
  isCompatible,
  getActiveFeatures,
  isFeatureActive,
  type FeatureFlag,
  type UpgradeProposal,
  type UpgradeSignal,
  type VersionInfo,
  type VersionCompatibility,
} from '@rinku/core';

export interface VersionServiceConfig {
  nodeVersion: string;
  chainId: string;
  networkId: string;
  minCompatibleVersion: string;
  proposalExpiryMs: number;
  signalWindowCheckpoints: number;
}

export interface VersionServiceSnapshot {
  features: FeatureFlag[];
  proposals: UpgradeProposal[];
  signals: UpgradeSignal[];
  activationHistory: { featureId: string; height: number; timestamp: number }[];
}

const DEFAULT_CONFIG: VersionServiceConfig = {
  nodeVersion: '0.1.0',
  chainId: 'rinku-mainnet',
  networkId: 'rinku',
  minCompatibleVersion: '0.1.0',
  proposalExpiryMs: 7 * 24 * 60 * 60 * 1000,
  signalWindowCheckpoints: 2016,
};

export class VersionService {
  private config: VersionServiceConfig;
  private features: Map<string, FeatureFlag> = new Map();
  private proposals: Map<string, UpgradeProposal> = new Map();
  private signals: Map<string, UpgradeSignal[]> = new Map();
  private activationHistory: { featureId: string; height: number; timestamp: number }[] = [];
  private currentHeight: number = 0;

  constructor(config: Partial<VersionServiceConfig> = {}) {
    this.config = { ...DEFAULT_CONFIG, ...config };
    this.initializeFeatures();
  }

  private initializeFeatures(): void {
    for (const [id, featureDef] of Object.entries(KNOWN_FEATURES)) {
      this.features.set(id, {
        ...featureDef,
        activationHeight: 0,
        status: 'active',
      });
    }
  }

  aggregateVersionSignals(
    checkpointSignatures: { validator: string; version?: string; supportedFeatures?: string[]; weight: number }[]
  ): { version: string; weight: number; count: number }[] {
    const signalMap = new Map<string, { weight: number; count: number }>();
    
    for (const sig of checkpointSignatures) {
      const version = sig.version || PROTOCOL_VERSION;
      const existing = signalMap.get(version) || { weight: 0, count: 0 };
      existing.weight += sig.weight;
      existing.count += 1;
      signalMap.set(version, existing);
    }

    return Array.from(signalMap.entries())
      .map(([version, data]) => ({ version, ...data }))
      .sort((a, b) => compareVersions(b.version, a.version));
  }

  processCheckpointSignals(
    checkpointSignatures: { validator: string; version?: string; supportedFeatures?: string[]; weight: number }[]
  ): void {
    for (const sig of checkpointSignatures) {
      if (!sig.version || !sig.supportedFeatures) continue;
      
      for (const proposal of this.proposals.values()) {
        if (proposal.status !== 'proposed' && proposal.status !== 'signaling') continue;
        if (proposal.targetVersion !== sig.version) continue;
        
        this.recordSignal(proposal.id, {
          validator: sig.validator,
          version: sig.version,
          features: sig.supportedFeatures,
          timestamp: Date.now(),
          signature: '',
        }, sig.weight);
      }
    }
  }

  getVersionInfo(): VersionInfo {
    return {
      protocolVersion: PROTOCOL_VERSION,
      nodeVersion: this.config.nodeVersion,
      chainId: this.config.chainId,
      networkId: this.config.networkId,
      features: Array.from(this.features.values()),
      minCompatibleVersion: this.config.minCompatibleVersion,
      activationHeight: this.currentHeight,
    };
  }

  checkPeerCompatibility(remoteVersion: string): VersionCompatibility {
    return isCompatible(
      PROTOCOL_VERSION,
      remoteVersion,
      this.config.minCompatibleVersion
    );
  }

  getActiveFeatures(): string[] {
    return getActiveFeatures(Array.from(this.features.values()), this.currentHeight);
  }

  isFeatureActive(featureId: string): boolean {
    return isFeatureActive(Array.from(this.features.values()), featureId, this.currentHeight);
  }

  proposeUpgrade(
    title: string,
    description: string,
    targetVersion: string,
    featureIds: string[],
    proposedBy: string,
    totalValidatorWeight: number
  ): UpgradeProposal | null {
    if (compareVersions(targetVersion, PROTOCOL_VERSION) <= 0) {
      return null;
    }

    for (const fid of featureIds) {
      if (!KNOWN_FEATURES[fid]) {
        return null;
      }
    }

    const proposalId = `prop-${targetVersion}-${Date.now().toString(36)}`;
    const proposal: UpgradeProposal = {
      id: proposalId,
      title,
      description,
      targetVersion,
      features: featureIds,
      proposedAt: Date.now(),
      proposedBy,
      activationThreshold: 0.75,
      activationHeight: null,
      signalCount: 0,
      signalWeight: 0,
      totalWeight: totalValidatorWeight,
      status: 'proposed',
      expiresAt: Date.now() + this.config.proposalExpiryMs,
    };

    this.proposals.set(proposalId, proposal);
    this.signals.set(proposalId, []);
    return proposal;
  }

  recordSignal(
    proposalId: string,
    signal: UpgradeSignal,
    validatorWeight: number
  ): boolean {
    const proposal = this.proposals.get(proposalId);
    if (!proposal || proposal.status !== 'proposed' && proposal.status !== 'signaling') {
      return false;
    }

    const signals = this.signals.get(proposalId) || [];
    if (signals.some(s => s.validator === signal.validator)) {
      return false;
    }

    signals.push(signal);
    this.signals.set(proposalId, signals);

    proposal.signalCount = signals.length;
    proposal.signalWeight += validatorWeight;
    proposal.status = 'signaling';

    if (proposal.signalWeight / proposal.totalWeight >= proposal.activationThreshold) {
      proposal.status = 'locked_in';
      proposal.activationHeight = this.currentHeight + this.config.signalWindowCheckpoints;
    }

    return true;
  }

  onCheckpoint(height: number, totalValidatorWeight: number): void {
    this.currentHeight = height;

    for (const proposal of this.proposals.values()) {
      proposal.totalWeight = totalValidatorWeight;

      if (proposal.status === 'proposed' || proposal.status === 'signaling') {
        if (Date.now() > proposal.expiresAt) {
          proposal.status = 'expired';
          continue;
        }
      }

      if (proposal.status === 'locked_in' && 
          proposal.activationHeight !== null && 
          height >= proposal.activationHeight) {
        this.activateProposal(proposal);
      }
    }
  }

  private activateProposal(proposal: UpgradeProposal): void {
    proposal.status = 'active';

    for (const featureId of proposal.features) {
      const feature = this.features.get(featureId);
      if (feature) {
        feature.status = 'active';
        feature.activationHeight = this.currentHeight;
        this.activationHistory.push({
          featureId,
          height: this.currentHeight,
          timestamp: Date.now(),
        });
      }
    }
  }

  getProposals(): UpgradeProposal[] {
    return Array.from(this.proposals.values());
  }

  getProposal(proposalId: string): UpgradeProposal | undefined {
    return this.proposals.get(proposalId);
  }

  getProposalSignals(proposalId: string): UpgradeSignal[] {
    return this.signals.get(proposalId) || [];
  }

  getActivationHistory(): { featureId: string; height: number; timestamp: number }[] {
    return [...this.activationHistory];
  }

  toJSON(): VersionServiceSnapshot {
    return {
      features: Array.from(this.features.values()),
      proposals: Array.from(this.proposals.values()),
      signals: Array.from(this.signals.entries()).flatMap(([, sigs]) => sigs),
      activationHistory: this.activationHistory,
    };
  }

  static fromJSON(data: VersionServiceSnapshot, config?: Partial<VersionServiceConfig>): VersionService {
    const service = new VersionService(config);
    
    service.features.clear();
    for (const feature of data.features) {
      service.features.set(feature.id, feature);
    }

    for (const proposal of data.proposals) {
      service.proposals.set(proposal.id, proposal);
    }

    const signalsByProposal = new Map<string, UpgradeSignal[]>();
    for (const signal of data.signals) {
      const version = signal.version;
      const matchingProposal = data.proposals.find(p => p.targetVersion === version);
      if (matchingProposal) {
        const arr = signalsByProposal.get(matchingProposal.id) || [];
        arr.push(signal);
        signalsByProposal.set(matchingProposal.id, arr);
      }
    }
    service.signals = signalsByProposal;

    service.activationHistory = data.activationHistory;
    return service;
  }
}
