import {
  type ContractState,
  type ContractDeploy,
  type ContractCall,
  type ContractTransaction,
  type ExecutionResult,
  type StateDiff,
  type ContractReceipt,
  type ContractEvent,
  type StateWitness,
  type ContractReceiptWithWitness,
  type TouchedKey,
  type SignedTransaction,
  type Checkpoint,
  type ContractReceiptProofB,
  createContractState,
  computeStateHash,
  computeStateDiff,
  createMockRuntime,
  validateContractCall,
  createContractURL,
  parseContractURL,
  type WasmHostBindings,
  type ExtendedExecutionResult,
  StateTrie,
  ReceiptsTrie,
  createContractReceipt,
  createReceiptWithWitness
} from '@rinku/core';
import { StateManager } from './state.js';

export class ContractService {
  private contracts: Map<string, ContractState> = new Map();
  private executionHistory: Map<string, StateDiff[]> = new Map();
  private receipts: Map<string, ContractReceipt> = new Map();
  private runtime = createMockRuntime();
  private stateTrie: StateTrie;
  private receiptsTrie: ReceiptsTrie;
  private currentCheckpointHeight: number = 0;

  constructor(private stateManager: StateManager) {
    this.stateTrie = new StateTrie();
    this.receiptsTrie = new ReceiptsTrie();
  }

  setCheckpointHeight(height: number): void {
    this.currentCheckpointHeight = height;
  }

  async deployContract(deploy: ContractDeploy): Promise<{
    success: boolean;
    contractId?: string;
    deployUrl?: string;
    error?: string;
  }> {
    if (this.contracts.has(deploy.contractId)) {
      return { success: false, error: 'Contract ID already exists' };
    }

    const deployUrl = createContractURL(deploy).path;
    const contractState = createContractState(deploy, deployUrl);
    
    this.contracts.set(deploy.contractId, contractState);
    this.executionHistory.set(deploy.contractId, []);

    return {
      success: true,
      contractId: deploy.contractId,
      deployUrl
    };
  }

  async executeCall(
    tx: ContractTransaction
  ): Promise<{ result: ExtendedExecutionResult; receipt?: ContractReceipt }> {
    const call = tx.contract;
    if (!call) {
      return {
        result: {
          success: false,
          stateDiff: null,
          gasUsed: 0,
          error: 'No contract call in transaction',
          logs: [],
          events: []
        }
      };
    }

    const contract = this.contracts.get(call.contractId);
    if (!contract) {
      return {
        result: {
          success: false,
          stateDiff: null,
          gasUsed: 0,
          error: `Contract not found: ${call.contractId}`,
          logs: [],
          events: []
        }
      };
    }

    const validation = validateContractCall(call, contract);
    if (!validation.valid) {
      return {
        result: {
          success: false,
          stateDiff: null,
          gasUsed: 0,
          error: validation.error,
          logs: [],
          events: []
        }
      };
    }

    const bindings = this.createHostBindings();
    const preStateRoot = await this.stateTrie.getRoot();
    
    const result = this.runtime.execute(
      call.contractId,
      contract.wasmBase64,
      call.entrypoint,
      call.input,
      contract.state,
      contract.height + 1,
      bindings
    );

    if (result.success && result.stateDiff) {
      const diff = result.stateDiff;

      if (diff.postHash !== call.postStateHash) {
        return {
          result: {
            success: false,
            stateDiff: null,
            gasUsed: result.gasUsed,
            error: `Post-state hash mismatch. Expected: ${call.postStateHash}, Got: ${diff.postHash}`,
            logs: result.logs,
            events: result.events
          }
        };
      }

      contract.state = this.applyDiffToState(contract.state, diff);
      contract.stateHash = diff.postHash;
      contract.height++;

      await this.stateTrie.setContractState(call.contractId, contract.state);
      const postStateRoot = await this.stateTrie.getRoot();

      const history = this.executionHistory.get(call.contractId) || [];
      history.push(diff);
      this.executionHistory.set(call.contractId, history);

      const events: ContractEvent[] = result.events.map((e, idx) => ({
        contractId: call.contractId,
        eventName: e.eventName,
        data: e.data,
        index: idx
      }));

      const receipt = await createContractReceipt({
        txHash: tx.hash,
        checkpointHeight: this.currentCheckpointHeight,
        contractId: call.contractId,
        entrypoint: call.entrypoint,
        caller: tx.from,
        preStateRoot,
        postStateRoot,
        stateDiff: diff,
        status: 'success',
        gasUsed: result.gasUsed,
        gasLimit: 1_000_000,
        events
      });

      this.receipts.set(receipt.callId, receipt);
      await this.receiptsTrie.addReceipt(receipt.callId, receipt);

      return {
        result: {
          success: true,
          stateDiff: diff,
          gasUsed: result.gasUsed,
          logs: result.logs,
          events: result.events
        },
        receipt
      };
    }

    if (!result.success) {
      const postStateRoot = await this.stateTrie.getRoot();
      
      const events: ContractEvent[] = result.events.map((e, idx) => ({
        contractId: call.contractId,
        eventName: e.eventName,
        data: e.data,
        index: idx
      }));

      const receipt = await createContractReceipt({
        txHash: tx.hash,
        checkpointHeight: this.currentCheckpointHeight,
        contractId: call.contractId,
        entrypoint: call.entrypoint,
        caller: tx.from,
        preStateRoot,
        postStateRoot,
        stateDiff: null,
        status: result.error?.includes('out of gas') ? 'out_of_gas' : 'revert',
        gasUsed: result.gasUsed,
        gasLimit: 1_000_000,
        events,
        revertReason: result.error
      });

      this.receipts.set(receipt.callId, receipt);
      await this.receiptsTrie.addReceipt(receipt.callId, receipt);

      return { result, receipt };
    }

    return { result };
  }

  private applyDiffToState(
    state: Record<string, unknown>,
    diff: StateDiff
  ): Record<string, unknown> {
    const newState = JSON.parse(JSON.stringify(state));
    
    for (const change of diff.changes) {
      if (change.newValue === undefined) {
        delete newState[change.key];
      } else {
        newState[change.key] = change.newValue;
      }
    }
    
    return newState;
  }

  private createHostBindings(): WasmHostBindings {
    return {
      getBalance: (address: string) => {
        const account = this.stateManager.getAccount(address);
        return account?.balance || 0;
      },
      getAccountAge: (address: string) => {
        const account = this.stateManager.getAccount(address);
        if (!account) return 0;
        return Math.floor((Date.now() - account.firstTxTimestamp) / (1000 * 60 * 60 * 24));
      },
      log: (message: string) => {
        console.log(`[Contract] ${message}`);
      },
      emit: (eventName: string, data: Record<string, unknown>) => {
        console.log(`[Contract Event] ${eventName}:`, data);
      },
      getCurrentTime: () => Date.now(),
      getBlockHeight: () => this.currentCheckpointHeight
    };
  }

  getReceipt(callId: string): ContractReceipt | undefined {
    return this.receipts.get(callId);
  }

  getAllReceipts(): ContractReceipt[] {
    return Array.from(this.receipts.values());
  }

  async getStateRoot(): Promise<string> {
    return this.stateTrie.getRoot();
  }

  async getReceiptRoot(): Promise<string> {
    return this.receiptsTrie.getRoot();
  }

  async getStateProof(contractId: string, key: string) {
    return this.stateTrie.getProof(contractId, key);
  }

  async getReceiptProof(callId: string) {
    return this.receiptsTrie.getProof(callId);
  }

  clearCheckpointReceipts(): void {
    this.receiptsTrie.clear();
  }

  getContract(contractId: string): ContractState | undefined {
    return this.contracts.get(contractId);
  }

  getAllContracts(): ContractState[] {
    return Array.from(this.contracts.values());
  }

  getContractState(contractId: string): Record<string, unknown> | undefined {
    const contract = this.contracts.get(contractId);
    return contract?.state;
  }

  getExecutionHistory(contractId: string): StateDiff[] {
    return this.executionHistory.get(contractId) || [];
  }

  simulateCall(
    contractId: string,
    entrypoint: string,
    input: Record<string, unknown>
  ): ExecutionResult {
    const contract = this.contracts.get(contractId);
    if (!contract) {
      return {
        success: false,
        stateDiff: null,
        gasUsed: 0,
        error: `Contract not found: ${contractId}`,
        logs: []
      };
    }

    const bindings = this.createHostBindings();
    
    return this.runtime.execute(
      contractId,
      contract.wasmBase64,
      entrypoint,
      input,
      contract.state,
      contract.height + 1,
      bindings
    );
  }

  async assembleWitness(receipt: ContractReceipt): Promise<StateWitness | null> {
    const contract = this.contracts.get(receipt.contractId);
    if (!contract) return null;

    const history = this.executionHistory.get(receipt.contractId) || [];
    const lastDiff = history[history.length - 1];
    
    const touchedKeys: TouchedKey[] = lastDiff?.changes.map(change => ({
      contractId: receipt.contractId,
      key: change.key,
      preValue: change.oldValue,
      postValue: change.newValue
    })) || [];

    const merkleProofs: StateWitness['merkleProofs'] = [];
    
    for (const tk of touchedKeys) {
      const proof = await this.stateTrie.getProof(tk.contractId, tk.key);
      if (proof) {
        merkleProofs.push({
          key: proof.key,
          proof: proof.proof,
          index: proof.index
        });
      }
    }

    return {
      touchedKeys,
      merkleProofs
    };
  }

  async getReceiptWithWitness(callId: string): Promise<ContractReceiptWithWitness | null> {
    const receipt = this.receipts.get(callId);
    if (!receipt) return null;

    const witness = await this.assembleWitness(receipt);
    if (!witness) return null;

    return createReceiptWithWitness(receipt, witness);
  }

  async assembleProfileBProof(
    callId: string,
    tx: SignedTransaction,
    checkpoint: Checkpoint
  ): Promise<ContractReceiptProofB | null> {
    const receipt = this.receipts.get(callId);
    if (!receipt) return null;

    const witness = await this.assembleWitness(receipt);
    if (!witness) return null;

    const receiptProof = await this.receiptsTrie.getProof(callId);
    if (!receiptProof) return null;

    const receiptRoot = await this.receiptsTrie.getRoot();

    const validatorSignatures = checkpoint.signatures.map(sig => ({
      validator: sig.validator,
      signature: sig.signature,
      weight: sig.weight
    }));

    return {
      receipt,
      tx,
      txHash: tx.hash,
      checkpointAnchor: {
        checkpointId: checkpoint.checkpointId,
        merkleRoot: checkpoint.merkleRoot,
        stateRoot: checkpoint.stateRoot,
        receiptRoot: checkpoint.receiptRoot,
        height: checkpoint.height,
        signatureCount: checkpoint.signatures.length
      },
      witness,
      validatorSignatures,
      receiptMerkleProof: {
        proof: receiptProof.proof,
        index: receiptProof.index,
        receiptRoot
      }
    };
  }

  toJSON(): object {
    return {
      contracts: Array.from(this.contracts.entries()).map(([id, contract]) => ({
        id,
        contract: {
          ...contract,
          state: contract.state
        }
      })),
      executionHistory: Array.from(this.executionHistory.entries()),
      stateTrie: this.stateTrie.toJSON(),
      receiptsTrie: this.receiptsTrie.toJSON()
    };
  }

  static async fromJSON(data: any, stateManager: StateManager): Promise<ContractService> {
    const service = new ContractService(stateManager);
    
    if (data.contracts) {
      for (const { id, contract } of data.contracts) {
        service.contracts.set(id, contract);
      }
    }
    
    if (data.executionHistory) {
      for (const [id, history] of data.executionHistory) {
        service.executionHistory.set(id, history);
      }
    }
    
    if (data.stateTrie) {
      service.stateTrie = await StateTrie.fromJSON(data.stateTrie);
    }
    
    if (data.receiptsTrie) {
      service.receiptsTrie = await ReceiptsTrie.fromJSON(data.receiptsTrie);
    }
    
    return service;
  }
}
