import { describe, it, expect } from 'vitest';
import {
  createCheckpoint,
  createGenesisCheckpoint,
  createCheckpointProof,
  verifyCheckpointProof,
  verifyCheckpointChain,
  computeValidatorSetHash,
  GENESIS_CHECKPOINT_ID,
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
        1,
        'abc123',
        [],
        100,
        1000,
        [],
        GENESIS_CHECKPOINT_ID
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
        1,
        'abc123',
        [],
        100,
        300,
        validators,
        GENESIS_CHECKPOINT_ID
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
        1,
        'abc123',
        [],
        10,
        100,
        validators,
        GENESIS_CHECKPOINT_ID
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
        1,
        'abc123',
        [],
        10,
        100,
        validators,
        GENESIS_CHECKPOINT_ID
      );
      
      const proof = createCheckpointProof(checkpoint, 0);
      const result = await verifyCheckpointProof(proof, validators);
      
      expect(result.valid).toBe(false);
      expect(result.errors.some(e => e.includes('Insufficient'))).toBe(true);
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
        1,
        'abc123',
        [],
        10,
        100,
        trustedValidators,
        GENESIS_CHECKPOINT_ID
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
        1,
        'abc123',
        [],
        10,
        100,
        trustedValidators,
        GENESIS_CHECKPOINT_ID
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
        1,
        'abc123',
        [],
        10,
        100,
        attackerValidators,
        GENESIS_CHECKPOINT_ID
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
        1,
        'abc123',
        [],
        10,
        100,
        trustedValidators,
        GENESIS_CHECKPOINT_ID
      );
      
      checkpoint = await signCheckpointManually(
        checkpoint,
        legitimateAddress,
        attacker.privateKey,
        attacker.publicKey,
        100
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
        1,
        'abc123',
        [],
        10,
        100,
        checkpointValidators,
        GENESIS_CHECKPOINT_ID
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
  });
});
