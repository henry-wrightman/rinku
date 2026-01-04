import { describe, it, expect } from 'vitest';
import {
  createCheckpoint,
  createGenesisCheckpoint,
  createCheckpointProof,
  verifyCheckpointProof,
  verifyCheckpointChain,
  computeValidatorSetHash,
  encodeCheckpointProof,
  decodeCheckpointProof,
  embedProofInUrl,
  extractProofFromUrl,
  GENESIS_CHECKPOINT_ID,
  createCheckpointCertificate,
  parseSignerBitmap,
  verifyCheckpointCertificate,
  encodeCheckpointCertificate,
  decodeCheckpointCertificate,
  estimateProfileBSize,
  getCheckpointSigningData,
  getBLSCheckpointSigningData,
} from '../checkpoint.js';
import { generateKeyPair, computeFingerprint, sign } from '../crypto.js';
import type { Checkpoint, CheckpointProof, ValidatorEntry, ValidatorSignature } from '../types.js';

async function signCheckpointManually(
  checkpoint: Checkpoint,
  address: string,
  privateKey: Uint8Array,
  publicKey: Uint8Array,
  weight: number
): Promise<Checkpoint> {
  const signingData = JSON.stringify({
    checkpointId: checkpoint.checkpointId,
    height: checkpoint.height,
    merkleRoot: checkpoint.merkleRoot,
    totalWeight: checkpoint.totalWeight,
    validatorSetHash: checkpoint.validatorSetHash,
    previousCheckpointId: checkpoint.previousCheckpointId
  }) + `:${weight}`;
  
  const signature = await sign(signingData, privateKey);
  
  const sig: ValidatorSignature = {
    validator: address,
    signature,
    publicKey: Array.from(publicKey),
    weight,
    timestamp: Date.now()
  };
  
  return {
    ...checkpoint,
    signatures: [...checkpoint.signatures, sig]
  };
}

describe('Checkpoint Module', () => {
  describe('Genesis Checkpoint', () => {
    it('should create genesis checkpoint with correct ID', async () => {
      const genesis = await createGenesisCheckpoint('rinku-testnet', []);
      expect(genesis.checkpointId).toBe(GENESIS_CHECKPOINT_ID);
      expect(genesis.height).toBe(0);
      expect(genesis.previousCheckpointId).toBeNull();
    });

    it('should include validators in genesis', async () => {
      const { publicKey } = await generateKeyPair();
      const address = await computeFingerprint(publicKey);
      const validators: ValidatorEntry[] = [
        { address, publicKey: Array.from(publicKey), weight: 100 }
      ];
      const genesis = await createGenesisCheckpoint('rinku-testnet', validators);
      expect(genesis.validators).toEqual(validators);
      expect(genesis.totalWeight).toBe(100);
    });
  });

  describe('Checkpoint Creation', () => {
    it('should create valid checkpoint', async () => {
      const checkpoint = await createCheckpoint(
        1, 'abc123', 0, 100, 1000, [], GENESIS_CHECKPOINT_ID
      );
      expect(checkpoint.checkpointId).toBeDefined();
      expect(checkpoint.height).toBe(1);
      expect(checkpoint.merkleRoot).toBe('abc123');
      expect(checkpoint.previousCheckpointId).toBe(GENESIS_CHECKPOINT_ID);
    });

    it('should include validator set hash', async () => {
      const validators: ValidatorEntry[] = [
        { address: 'validator1', publicKey: [1, 2, 3], weight: 100 },
        { address: 'validator2', publicKey: [4, 5, 6], weight: 200 },
      ];
      const checkpoint = await createCheckpoint(
        1, 'abc123', 0, 100, 300, validators, GENESIS_CHECKPOINT_ID
      );
      expect(checkpoint.validatorSetHash).toBeDefined();
      expect(checkpoint.validators).toEqual(validators);
    });
  });

  describe('Validator Set Hash', () => {
    it('should compute deterministic hash', async () => {
      const validators: ValidatorEntry[] = [
        { address: 'v1', publicKey: [1, 2, 3], weight: 100 },
        { address: 'v2', publicKey: [4, 5, 6], weight: 200 },
      ];
      const hash1 = await computeValidatorSetHash(validators);
      const hash2 = await computeValidatorSetHash(validators);
      expect(hash1).toBe(hash2);
    });

    it('should produce different hashes for different validators', async () => {
      const v1: ValidatorEntry[] = [{ address: 'v1', publicKey: [1], weight: 100 }];
      const v2: ValidatorEntry[] = [{ address: 'v2', publicKey: [2], weight: 100 }];
      const hash1 = await computeValidatorSetHash(v1);
      const hash2 = await computeValidatorSetHash(v2);
      expect(hash1).not.toBe(hash2);
    });
  });

  describe('Proof Verification', () => {
    it('should verify valid proof against trusted validators', async () => {
      const { publicKey, privateKey } = await generateKeyPair();
      const address = await computeFingerprint(publicKey);
      const pubKeyArray = Array.from(publicKey);
      const validators: ValidatorEntry[] = [
        { address, publicKey: pubKeyArray, weight: 100 }
      ];
      let checkpoint = await createCheckpoint(
        1, 'abc123', 0, 10, 100, validators, GENESIS_CHECKPOINT_ID
      );
      checkpoint = await signCheckpointManually(checkpoint, address, privateKey, publicKey, 100);
      const proof = createCheckpointProof(checkpoint, 100);
      const result = await verifyCheckpointProof(proof, validators);
      expect(result.valid).toBe(true);
      expect(result.signatureCount).toBe(1);
      expect(result.validatorWeightPercent).toBe(100);
    });

    it('should reject proof with insufficient signatures', async () => {
      const validators: ValidatorEntry[] = [
        { address: 'v1', publicKey: [1, 2, 3], weight: 100 }
      ];
      const checkpoint = await createCheckpoint(
        1, 'abc123', 0, 10, 100, validators, GENESIS_CHECKPOINT_ID
      );
      const proof = createCheckpointProof(checkpoint, 0);
      const result = await verifyCheckpointProof(proof, validators);
      expect(result.valid).toBe(false);
      expect(result.errors.some(e => e.includes('Insufficient'))).toBe(true);
    });
  });

  describe('Proof Encoding', () => {
    it('should encode and decode proof', async () => {
      const { publicKey, privateKey } = await generateKeyPair();
      const address = await computeFingerprint(publicKey);
      const pubKeyArray = Array.from(publicKey);
      const validators: ValidatorEntry[] = [
        { address, publicKey: pubKeyArray, weight: 100 }
      ];
      let checkpoint = await createCheckpoint(
        1, 'abc123', 0, 10, 100, validators, GENESIS_CHECKPOINT_ID
      );
      checkpoint = await signCheckpointManually(checkpoint, address, privateKey, publicKey, 100);
      const proof = createCheckpointProof(checkpoint, 100);
      const encoded = encodeCheckpointProof(proof);
      const decoded = decodeCheckpointProof(encoded);
      expect(decoded).not.toBeNull();
      expect(decoded!.checkpointId).toBe(proof.checkpointId);
      expect(decoded!.checkpointHeight).toBe(proof.checkpointHeight);
      expect(decoded!.merkleRoot).toBe(proof.merkleRoot);
    });

    it('should return null for invalid encoded proof', () => {
      expect(decodeCheckpointProof('invalid!!!')).toBeNull();
    });
  });

  describe('URL Proof Embedding', () => {
    it('should embed proof in URL', async () => {
      const proof: CheckpointProof = {
        checkpointId: 'cp_001',
        checkpointHeight: 1,
        merkleRoot: 'abc123',
        signatureCount: 1,
        totalValidatorWeight: 100,
        totalNetworkWeight: 100,
        validatorSetHash: 'vsh123',
        previousCheckpointId: GENESIS_CHECKPOINT_ID,
        validators: [],
        signatures: []
      };
      const url = embedProofInUrl('/tx/payload123', proof);
      expect(url).toContain('/tx/payload123');
      expect(url).toContain('?proof=');
    });

    it('should handle URL with existing query params', async () => {
      const proof: CheckpointProof = {
        checkpointId: 'cp_001',
        checkpointHeight: 1,
        merkleRoot: 'abc123',
        signatureCount: 1,
        totalValidatorWeight: 100,
        totalNetworkWeight: 100,
        validatorSetHash: 'vsh123',
        previousCheckpointId: null,
        validators: [],
        signatures: []
      };
      const url = embedProofInUrl('/tx/payload123?existing=param', proof);
      expect(url).toContain('&proof=');
    });

    it('should extract proof from URL', async () => {
      const proof: CheckpointProof = {
        checkpointId: 'cp_001',
        checkpointHeight: 1,
        merkleRoot: 'abc123',
        signatureCount: 1,
        totalValidatorWeight: 100,
        totalNetworkWeight: 100,
        validatorSetHash: 'vsh123',
        previousCheckpointId: null,
        validators: [],
        signatures: []
      };
      const url = embedProofInUrl('/tx/payload123', proof);
      const extracted = extractProofFromUrl(url);
      expect(extracted.proof).not.toBeNull();
      expect(extracted.proof!.checkpointId).toBe('cp_001');
      expect(extracted.txUrl).toBe('/tx/payload123');
    });

    it('should handle URL without proof', () => {
      const extracted = extractProofFromUrl('/tx/payload123');
      expect(extracted.proof).toBeNull();
      expect(extracted.txUrl).toBe('/tx/payload123');
    });
  });

  describe('Checkpoint Chain Verification', () => {
    it('should verify genesis checkpoint directly', async () => {
      const { publicKey, privateKey } = await generateKeyPair();
      const address = await computeFingerprint(publicKey);
      const validators: ValidatorEntry[] = [
        { address, publicKey: Array.from(publicKey), weight: 100 }
      ];
      let checkpoint = await createCheckpoint(
        0, 'genesis', 0, 10, 100, validators, null
      );
      checkpoint.checkpointId = GENESIS_CHECKPOINT_ID;
      checkpoint = await signCheckpointManually(checkpoint, address, privateKey, publicKey, 100);
      const proof = createCheckpointProof(checkpoint, 100);
      proof.checkpointHeight = 0;
      const genesisConfig = { 
        chainId: 'testnet', 
        initialValidators: validators,
        genesisTime: Date.now(),
        genesisCheckpointId: GENESIS_CHECKPOINT_ID
      };
      const result = await verifyCheckpointChain(proof, [], genesisConfig);
      expect(result.valid).toBe(true);
    });

    it('should verify checkpoint chain from genesis', async () => {
      const { publicKey, privateKey } = await generateKeyPair();
      const address = await computeFingerprint(publicKey);
      const validators: ValidatorEntry[] = [
        { address, publicKey: Array.from(publicKey), weight: 100 }
      ];
      let checkpoint1 = await createCheckpoint(
        1, 'merkle1', 0, 10, 100, validators, GENESIS_CHECKPOINT_ID
      );
      checkpoint1 = await signCheckpointManually(checkpoint1, address, privateKey, publicKey, 100);
      const proof1 = createCheckpointProof(checkpoint1, 100);
      let checkpoint2 = await createCheckpoint(
        2, 'merkle2', 0, 20, 100, validators, checkpoint1.checkpointId
      );
      checkpoint2 = await signCheckpointManually(checkpoint2, address, privateKey, publicKey, 100);
      const proof2 = createCheckpointProof(checkpoint2, 100);
      const genesisConfig = { 
        chainId: 'testnet', 
        initialValidators: validators,
        genesisTime: Date.now(),
        genesisCheckpointId: GENESIS_CHECKPOINT_ID
      };
      const result = await verifyCheckpointChain(proof2, [proof1], genesisConfig);
      expect(result.valid).toBe(true);
    });
  });

  describe('Security: Weight Inflation Attack Prevention', () => {
    it('should reject proof with inflated totalNetworkWeight', async () => {
      const { publicKey, privateKey } = await generateKeyPair();
      const address = await computeFingerprint(publicKey);
      const pubKeyArray = Array.from(publicKey);
      const trustedValidators: ValidatorEntry[] = [
        { address, publicKey: pubKeyArray, weight: 100 }
      ];
      let checkpoint = await createCheckpoint(
        1, 'abc123', 0, 10, 100, trustedValidators, GENESIS_CHECKPOINT_ID
      );
      checkpoint = await signCheckpointManually(checkpoint, address, privateKey, publicKey, 100);
      const proof = createCheckpointProof(checkpoint, 100);
      (proof as any).totalNetworkWeight = 1000;
      const result = await verifyCheckpointProof(proof, trustedValidators);
      expect(result.valid).toBe(false);
      expect(result.errors.some(e => e.toLowerCase().includes('weight mismatch'))).toBe(true);
    });

    it('should reject proof with lowered totalNetworkWeight (quorum bypass)', async () => {
      const { publicKey, privateKey } = await generateKeyPair();
      const address = await computeFingerprint(publicKey);
      const pubKeyArray = Array.from(publicKey);
      const trustedValidators: ValidatorEntry[] = [
        { address, publicKey: pubKeyArray, weight: 40 },
        { address: 'other_validator', publicKey: [1, 2, 3], weight: 60 },
      ];
      let checkpoint = await createCheckpoint(
        1, 'abc123', 0, 10, 100, trustedValidators, GENESIS_CHECKPOINT_ID
      );
      checkpoint = await signCheckpointManually(checkpoint, address, privateKey, publicKey, 40);
      const proof = createCheckpointProof(checkpoint, 40);
      (proof as any).totalNetworkWeight = 40;
      const result = await verifyCheckpointProof(proof, trustedValidators);
      expect(result.valid).toBe(false);
    });

    it('should reject proof from unknown validator', async () => {
      const { publicKey, privateKey } = await generateKeyPair();
      const address = await computeFingerprint(publicKey);
      const pubKeyArray = Array.from(publicKey);
      const attackerValidators: ValidatorEntry[] = [
        { address, publicKey: pubKeyArray, weight: 100 }
      ];
      let checkpoint = await createCheckpoint(
        1, 'abc123', 0, 10, 100, attackerValidators, GENESIS_CHECKPOINT_ID
      );
      checkpoint = await signCheckpointManually(checkpoint, address, privateKey, publicKey, 100);
      const proof = createCheckpointProof(checkpoint, 100);
      const trustedValidators: ValidatorEntry[] = [
        { address: 'legitimate_validator', publicKey: [9, 8, 7], weight: 100 }
      ];
      const result = await verifyCheckpointProof(proof, trustedValidators);
      expect(result.valid).toBe(false);
    });

    it('should reject forged signature (wrong private key)', async () => {
      const legitimate = await generateKeyPair();
      const attacker = await generateKeyPair();
      const legitimateAddress = await computeFingerprint(legitimate.publicKey);
      const trustedValidators: ValidatorEntry[] = [
        { address: legitimateAddress, publicKey: Array.from(legitimate.publicKey), weight: 100 }
      ];
      let checkpoint = await createCheckpoint(
        1, 'abc123', 0, 10, 100, trustedValidators, GENESIS_CHECKPOINT_ID
      );
      checkpoint = await signCheckpointManually(
        checkpoint, legitimateAddress, attacker.privateKey, attacker.publicKey, 100
      );
      const proof = createCheckpointProof(checkpoint, 100);
      const result = await verifyCheckpointProof(proof, trustedValidators);
      expect(result.valid).toBe(false);
    });

    it('should reject validator weight mismatch', async () => {
      const { publicKey, privateKey } = await generateKeyPair();
      const address = await computeFingerprint(publicKey);
      const pubKeyArray = Array.from(publicKey);
      const checkpointValidators: ValidatorEntry[] = [
        { address, publicKey: pubKeyArray, weight: 100 }
      ];
      let checkpoint = await createCheckpoint(
        1, 'abc123', 0, 10, 100, checkpointValidators, GENESIS_CHECKPOINT_ID
      );
      checkpoint = await signCheckpointManually(checkpoint, address, privateKey, publicKey, 100);
      const proof = createCheckpointProof(checkpoint, 100);
      const trustedValidators: ValidatorEntry[] = [
        { address, publicKey: pubKeyArray, weight: 50 }
      ];
      const result = await verifyCheckpointProof(proof, trustedValidators);
      expect(result.valid).toBe(false);
      expect(result.errors.some(e => e.toLowerCase().includes('weight'))).toBe(true);
    });

    it('should reject public key mismatch', async () => {
      const { publicKey, privateKey } = await generateKeyPair();
      const otherKey = await generateKeyPair();
      const address = await computeFingerprint(publicKey);
      const trustedValidators: ValidatorEntry[] = [
        { address, publicKey: Array.from(otherKey.publicKey), weight: 100 }
      ];
      let checkpoint = await createCheckpoint(
        1, 'abc123', 0, 10, 100, trustedValidators, GENESIS_CHECKPOINT_ID
      );
      checkpoint = await signCheckpointManually(checkpoint, address, privateKey, publicKey, 100);
      const proof = createCheckpointProof(checkpoint, 100);
      const result = await verifyCheckpointProof(proof, trustedValidators);
      expect(result.valid).toBe(false);
    });
  });

  describe('Checkpoint Signing Data', () => {
    it('should generate consistent signing data', async () => {
      const checkpoint = await createCheckpoint(
        1, 'merkle123', 5, 100, 1000, [], GENESIS_CHECKPOINT_ID
      );
      const data1 = getCheckpointSigningData(checkpoint);
      const data2 = getCheckpointSigningData(checkpoint);
      expect(data1).toBe(data2);
    });

    it('should generate BLS signing data', async () => {
      const checkpoint = await createCheckpoint(
        1, 'merkle123', 5, 100, 1000, [], GENESIS_CHECKPOINT_ID,
        'txMerkle123', [], 'state123', 'receipt123'
      );
      const data = getBLSCheckpointSigningData(checkpoint, 'sumTreeRoot', 500);
      expect(data).toContain(checkpoint.checkpointId);
      expect(data).toContain('sumTreeRoot');
      expect(data).toContain('500');
    });
  });

  describe('Profile B Checkpoint Certificates', () => {
    it('should create checkpoint certificate', async () => {
      const { publicKey, privateKey } = await generateKeyPair();
      const address = await computeFingerprint(publicKey);
      const validators: ValidatorEntry[] = [
        { address, publicKey: Array.from(publicKey), weight: 100 }
      ];
      let checkpoint = await createCheckpoint(
        1, 'merkle123', 5, 100, 100, validators, GENESIS_CHECKPOINT_ID
      );
      checkpoint = await signCheckpointManually(checkpoint, address, privateKey, publicKey, 100);
      const cert = createCheckpointCertificate(checkpoint, 'state123', 'receipt123');
      expect(cert.checkpointId).toBe(checkpoint.checkpointId);
      expect(cert.stateRoot).toBe('state123');
      expect(cert.receiptRoot).toBe('receipt123');
      expect(cert.signerBitmap).toBeDefined();
      expect(cert.signatures.length).toBe(1);
    });

    it('should parse signer bitmap', () => {
      const bitmap = '05';
      const indices = parseSignerBitmap(bitmap);
      expect(indices).toContain(0);
      expect(indices).toContain(2);
      expect(indices.length).toBe(2);
    });

    it('should handle multi-byte bitmap', () => {
      const bitmap = '0101';
      const indices = parseSignerBitmap(bitmap);
      expect(indices).toContain(0);
      expect(indices).toContain(8);
    });

    it('should encode and decode checkpoint certificate', async () => {
      const { publicKey, privateKey } = await generateKeyPair();
      const address = await computeFingerprint(publicKey);
      const validators: ValidatorEntry[] = [
        { address, publicKey: Array.from(publicKey), weight: 100 }
      ];
      let checkpoint = await createCheckpoint(
        1, 'merkle123', 5, 100, 100, validators, GENESIS_CHECKPOINT_ID
      );
      checkpoint = await signCheckpointManually(checkpoint, address, privateKey, publicKey, 100);
      const cert = createCheckpointCertificate(checkpoint, 'state123', 'receipt123');
      const encoded = encodeCheckpointCertificate(cert);
      expect(typeof encoded).toBe('string');
      const decoded = decodeCheckpointCertificate(encoded);
      expect(decoded).not.toBeNull();
      expect(decoded!.checkpointId).toBe(cert.checkpointId);
      expect(decoded!.stateRoot).toBe('state123');
      expect(decoded!.signatures.length).toBe(1);
    });

    it('should return null for invalid encoded certificate', () => {
      expect(decodeCheckpointCertificate('invalid!!!')).toBeNull();
    });

    it('should verify checkpoint certificate with valid signatures', async () => {
      const { publicKey, privateKey } = await generateKeyPair();
      const address = await computeFingerprint(publicKey);
      const validators: ValidatorEntry[] = [
        { address, publicKey: Array.from(publicKey), weight: 100 }
      ];
      let checkpoint = await createCheckpoint(
        1, 'merkle123', 5, 100, 100, validators, GENESIS_CHECKPOINT_ID
      );
      const certSigningData = JSON.stringify({
        checkpointId: checkpoint.checkpointId,
        height: checkpoint.height,
        merkleRoot: checkpoint.merkleRoot,
        stateRoot: 'state123',
        receiptRoot: 'receipt123',
        totalWeight: checkpoint.totalWeight,
        validatorSetHash: checkpoint.validatorSetHash
      }) + `:100`;
      const sig = await sign(certSigningData, privateKey);
      checkpoint.signatures = [{
        validator: address,
        signature: sig,
        publicKey: Array.from(publicKey),
        weight: 100,
        timestamp: Date.now()
      }];
      const cert = createCheckpointCertificate(checkpoint, 'state123', 'receipt123');
      const result = await verifyCheckpointCertificate(cert, validators);
      expect(result.valid).toBe(true);
      expect(result.weightPercent).toBe(100);
    });

    it('should reject certificate with insufficient weight', async () => {
      const { publicKey, privateKey } = await generateKeyPair();
      const address = await computeFingerprint(publicKey);
      const validators: ValidatorEntry[] = [
        { address, publicKey: Array.from(publicKey), weight: 30 },
        { address: 'other', publicKey: [1, 2, 3], weight: 70 }
      ];
      let checkpoint = await createCheckpoint(
        1, 'merkle123', 5, 100, 100, validators, GENESIS_CHECKPOINT_ID
      );
      const certSigningData = JSON.stringify({
        checkpointId: checkpoint.checkpointId,
        height: checkpoint.height,
        merkleRoot: checkpoint.merkleRoot,
        stateRoot: 'state123',
        receiptRoot: 'receipt123',
        totalWeight: checkpoint.totalWeight,
        validatorSetHash: checkpoint.validatorSetHash
      }) + `:30`;
      const sig = await sign(certSigningData, privateKey);
      checkpoint.signatures = [{
        validator: address,
        signature: sig,
        publicKey: Array.from(publicKey),
        weight: 30,
        timestamp: Date.now()
      }];
      const cert = createCheckpointCertificate(checkpoint, 'state123', 'receipt123');
      const result = await verifyCheckpointCertificate(cert, validators);
      expect(result.valid).toBe(false);
      expect(result.errors.some(e => e.includes('Insufficient'))).toBe(true);
    });

    it('should detect bitmap/signature count mismatch', async () => {
      const validators: ValidatorEntry[] = [
        { address: 'val0', publicKey: [1], weight: 100 }
      ];
      const cert = {
        checkpointId: 'cp123',
        merkleRoot: 'merkle',
        stateRoot: 'state',
        receiptRoot: 'receipt',
        height: 1,
        validatorSetHash: 'hash',
        totalWeight: 100,
        signerBitmap: '03',
        signatures: [{ idx: 0, sig: 'sig' }]
      };
      const result = await verifyCheckpointCertificate(cert, validators);
      expect(result.valid).toBe(false);
      expect(result.errors.some(e => e.includes('mismatch'))).toBe(true);
    });
  });

  describe('Profile B Size Estimation', () => {
    it('should estimate sizes for different validator counts', () => {
      const est = estimateProfileBSize(21, 15);
      expect(est.fullSignatures).toBeGreaterThan(0);
      expect(est.compactSignatures).toBeGreaterThan(0);
      expect(est.compactSignatures).toBeLessThan(est.fullSignatures);
    });

    it('should show savings with compact signatures', () => {
      const est = estimateProfileBSize(100, 67);
      expect(est.compactSignatures).toBeLessThan(est.fullSignatures);
    });
  });
});
