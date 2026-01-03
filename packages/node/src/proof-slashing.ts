import { 
  verifyProfileBProofCryptographic,
  verifyStateWitness,
  type ContractReceiptProofB,
  type ValidatorEntry,
  type CheckpointConfig,
  DEFAULT_CHECKPOINT_CONFIG
} from '@rinku/core';
import type { SlashingService, SlashReason, SlashEvent } from './tokenomics.js';
import type { ValidatorKeyManager } from './validator-keys.js';

export interface ProofViolation {
  type: SlashReason;
  validator: string;
  proof: ContractReceiptProofB;
  details: string;
  detectedAt: number;
}

export interface ProofValidationResult {
  valid: boolean;
  violations: ProofViolation[];
  errors: string[];
}

export interface ProofSlashingServiceDeps {
  slashingService: SlashingService;
  keyManager: ValidatorKeyManager;
  getCurrentCheckpointHeight: () => number;
  getCheckpointConfig: () => CheckpointConfig;
}

export class ProofSlashingService {
  private pendingViolations: ProofViolation[] = [];
  private processedViolations: Set<string> = new Set();
  private violationHistory: ProofViolation[] = [];

  constructor(private deps: ProofSlashingServiceDeps) {}

  async validateAndSlashIfInvalid(
    proof: ContractReceiptProofB,
    submittingValidator?: string
  ): Promise<ProofValidationResult> {
    const violations: ProofViolation[] = [];
    const errors: string[] = [];

    const trustedValidators = this.deps.keyManager.getRegisteredValidators();
    const config = this.deps.getCheckpointConfig();

    const result = await verifyProfileBProofCryptographic(proof, trustedValidators, config);

    if (!result.valid) {
      for (const error of result.errors) {
        const violation = this.categorizeError(error, proof, submittingValidator);
        if (violation) {
          violations.push(violation);
        }
        errors.push(error);
      }
    }

    for (const violation of violations) {
      await this.recordViolation(violation);
    }

    return {
      valid: result.valid,
      violations,
      errors,
    };
  }

  private categorizeError(
    error: string,
    proof: ContractReceiptProofB,
    submittingValidator?: string
  ): ProofViolation | null {
    const validator = submittingValidator || 'unknown';

    if (error.includes('State root mismatch') || error.includes('State witness verification')) {
      return {
        type: 'invalid_witness',
        validator,
        proof,
        details: error,
        detectedAt: Date.now(),
      };
    }

    if (error.includes('Receipt root mismatch') || error.includes('Receipt Merkle proof')) {
      return {
        type: 'receipt_tampering',
        validator,
        proof,
        details: error,
        detectedAt: Date.now(),
      };
    }

    if (error.includes('Invalid signature') || error.includes('Unknown validator') || 
        error.includes('Duplicate signature') || error.includes('Weight mismatch')) {
      return {
        type: 'invalid_proof',
        validator,
        proof,
        details: error,
        detectedAt: Date.now(),
      };
    }

    if (error.includes('Validator set hash mismatch')) {
      return {
        type: 'invalid_checkpoint',
        validator,
        proof,
        details: error,
        detectedAt: Date.now(),
      };
    }

    return null;
  }

  async recordViolation(violation: ProofViolation): Promise<void> {
    const violationId = this.getViolationId(violation);
    
    if (this.processedViolations.has(violationId)) {
      return;
    }

    this.pendingViolations.push(violation);
    this.violationHistory.push(violation);

    if (this.violationHistory.length > 1000) {
      this.violationHistory = this.violationHistory.slice(-500);
    }
  }

  private getViolationId(violation: ProofViolation): string {
    return `${violation.validator}:${violation.type}:${violation.proof.receipt?.callId || 'unknown'}`;
  }

  async processPendingViolations(): Promise<SlashEvent[]> {
    const slashEvents: SlashEvent[] = [];
    const checkpointHeight = this.deps.getCurrentCheckpointHeight();

    for (const violation of this.pendingViolations) {
      const violationId = this.getViolationId(violation);
      
      if (this.processedViolations.has(violationId)) {
        continue;
      }

      if (violation.validator === 'unknown') {
        this.processedViolations.add(violationId);
        continue;
      }

      const event = await this.deps.slashingService.slashValidator(
        violation.validator,
        violation.type,
        checkpointHeight,
        violation.details
      );

      if (event) {
        slashEvents.push(event);
      }

      this.processedViolations.add(violationId);
    }

    this.pendingViolations = [];
    return slashEvents;
  }

  async detectDoubleSignature(
    validatorAddress: string,
    signature1: { checkpoint: string; signature: string; timestamp: number },
    signature2: { checkpoint: string; signature: string; timestamp: number }
  ): Promise<SlashEvent | null> {
    if (signature1.checkpoint === signature2.checkpoint) {
      return null;
    }

    const checkpointHeight = this.deps.getCurrentCheckpointHeight();
    
    return this.deps.slashingService.slashValidator(
      validatorAddress,
      'double_sign',
      checkpointHeight,
      `Double signed checkpoints: ${signature1.checkpoint.slice(0, 8)}... and ${signature2.checkpoint.slice(0, 8)}...`
    );
  }

  async validateWitnessIntegrity(
    witness: any,
    expectedStateRoot: string,
    submittingValidator?: string
  ): Promise<{ valid: boolean; slashEvent?: SlashEvent }> {
    const result = await verifyStateWitness(witness, expectedStateRoot);

    if (!result.valid && submittingValidator) {
      const checkpointHeight = this.deps.getCurrentCheckpointHeight();
      const slashEvent = await this.deps.slashingService.slashValidator(
        submittingValidator,
        'invalid_witness',
        checkpointHeight,
        `Invalid witness: ${result.errors.join(', ')}`
      );
      return { valid: false, slashEvent: slashEvent || undefined };
    }

    return { valid: result.valid };
  }

  getPendingViolationsCount(): number {
    return this.pendingViolations.length;
  }

  getViolationHistory(): ProofViolation[] {
    return [...this.violationHistory];
  }

  getViolationsByValidator(validator: string): ProofViolation[] {
    return this.violationHistory.filter(v => v.validator === validator);
  }

  getViolationsByType(type: SlashReason): ProofViolation[] {
    return this.violationHistory.filter(v => v.type === type);
  }

  toJSON(): {
    pendingViolations: ProofViolation[];
    processedViolations: string[];
    violationHistory: ProofViolation[];
  } {
    return {
      pendingViolations: this.pendingViolations,
      processedViolations: Array.from(this.processedViolations),
      violationHistory: this.violationHistory,
    };
  }

  static fromJSON(
    data: {
      pendingViolations?: ProofViolation[];
      processedViolations?: string[];
      violationHistory?: ProofViolation[];
    },
    deps: ProofSlashingServiceDeps
  ): ProofSlashingService {
    const service = new ProofSlashingService(deps);

    if (data.pendingViolations) {
      service.pendingViolations = data.pendingViolations;
    }

    if (data.processedViolations) {
      for (const id of data.processedViolations) {
        service.processedViolations.add(id);
      }
    }

    if (data.violationHistory) {
      service.violationHistory = data.violationHistory;
    }

    return service;
  }
}
