export const PROTOCOL_VERSION = '1.0.0';

export interface ProtocolVersion {
  major: number;
  minor: number;
  patch: number;
}

export interface FeatureFlag {
  id: string;
  name: string;
  description: string;
  activationHeight: number | null;
  activationThreshold: number;
  status: 'proposed' | 'signaling' | 'locked_in' | 'active' | 'rejected';
}

export interface UpgradeSignal {
  validator: string;
  version: string;
  features: string[];
  timestamp: number;
  signature: string;
}

export interface VersionInfo {
  protocolVersion: string;
  nodeVersion: string;
  chainId: string;
  networkId: string;
  features: FeatureFlag[];
  minCompatibleVersion: string;
  activationHeight: number;
}

export interface UpgradeProposal {
  id: string;
  title: string;
  description: string;
  targetVersion: string;
  features: string[];
  proposedAt: number;
  proposedBy: string;
  activationThreshold: number;
  activationHeight: number | null;
  signalCount: number;
  signalWeight: number;
  totalWeight: number;
  status: 'proposed' | 'signaling' | 'locked_in' | 'active' | 'rejected' | 'expired';
  expiresAt: number;
}

export interface VersionCompatibility {
  compatible: boolean;
  localVersion: string;
  remoteVersion: string;
  reason?: string;
  canConnect: boolean;
  canSync: boolean;
}

export const KNOWN_FEATURES: Record<string, Omit<FeatureFlag, 'activationHeight' | 'status'>> = {
  'bls-aggregation': {
    id: 'bls-aggregation',
    name: 'BLS Signature Aggregation',
    description: 'Use BLS12-381 aggregated signatures for compact checkpoint proofs',
    activationThreshold: 0.75,
  },
  'zk-privacy': {
    id: 'zk-privacy',
    name: 'ZK Privacy Layer',
    description: 'Enable optional privacy-preserving proofs using Groth16 ZK-SNARKs',
    activationThreshold: 0.80,
  },
  'dynamic-gas': {
    id: 'dynamic-gas',
    name: 'EIP-1559 Style Gas',
    description: 'Utilization-based gas pricing that adjusts per checkpoint period',
    activationThreshold: 0.67,
  },
  'merkle-sum-tree': {
    id: 'merkle-sum-tree',
    name: 'MerkleSumTree Validator Proofs',
    description: 'Use MerkleSumTree for validator weight commitment and membership proofs',
    activationThreshold: 0.75,
  },
  'contract-receipts': {
    id: 'contract-receipts',
    name: 'Contract Execution Receipts',
    description: 'Include execution receipts in checkpoints for contract call verification',
    activationThreshold: 0.67,
  },
  'adaptive-fee-burn': {
    id: 'adaptive-fee-burn',
    name: 'Adaptive Fee Burn',
    description: 'Dynamic fee split with 70%+ validator floor and up to 30% burned',
    activationThreshold: 0.75,
  },
};

export function parseVersion(version: string): ProtocolVersion {
  const parts = version.split('.').map(Number);
  return {
    major: parts[0] || 0,
    minor: parts[1] || 0,
    patch: parts[2] || 0,
  };
}

export function formatVersion(version: ProtocolVersion): string {
  return `${version.major}.${version.minor}.${version.patch}`;
}

export function compareVersions(a: string, b: string): number {
  const va = parseVersion(a);
  const vb = parseVersion(b);
  
  if (va.major !== vb.major) return va.major - vb.major;
  if (va.minor !== vb.minor) return va.minor - vb.minor;
  return va.patch - vb.patch;
}

export function isCompatible(local: string, remote: string, minCompatible: string): VersionCompatibility {
  const localParsed = parseVersion(local);
  const remoteParsed = parseVersion(remote);
  const minParsed = parseVersion(minCompatible);
  
  if (remoteParsed.major !== localParsed.major) {
    return {
      compatible: false,
      localVersion: local,
      remoteVersion: remote,
      reason: `Major version mismatch: ${localParsed.major} vs ${remoteParsed.major}`,
      canConnect: false,
      canSync: false,
    };
  }
  
  if (compareVersions(remote, minCompatible) < 0) {
    return {
      compatible: false,
      localVersion: local,
      remoteVersion: remote,
      reason: `Remote version ${remote} is below minimum compatible version ${minCompatible}`,
      canConnect: true,
      canSync: false,
    };
  }
  
  const canSync = localParsed.minor === remoteParsed.minor || 
                  Math.abs(localParsed.minor - remoteParsed.minor) <= 1;
  
  return {
    compatible: true,
    localVersion: local,
    remoteVersion: remote,
    canConnect: true,
    canSync,
  };
}

export function getActiveFeatures(features: FeatureFlag[], currentHeight: number): string[] {
  return features
    .filter(f => f.status === 'active' || 
            (f.activationHeight !== null && currentHeight >= f.activationHeight))
    .map(f => f.id);
}

export function isFeatureActive(features: FeatureFlag[], featureId: string, currentHeight: number): boolean {
  const feature = features.find(f => f.id === featureId);
  if (!feature) return false;
  if (feature.status === 'active') return true;
  if (feature.activationHeight !== null && currentHeight >= feature.activationHeight) return true;
  return false;
}
