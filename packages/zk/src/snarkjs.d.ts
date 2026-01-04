declare module 'snarkjs' {
  export interface Groth16Proof {
    pi_a: [string, string, string];
    pi_b: [[string, string], [string, string], [string, string]];
    pi_c: [string, string, string];
    protocol: string;
    curve: string;
  }

  export namespace groth16 {
    function fullProve(
      input: Record<string, string | string[] | number[]>,
      wasmFile: string,
      zkeyFile: string
    ): Promise<{ proof: Groth16Proof; publicSignals: string[] }>;

    function verify(
      vkey: object,
      publicSignals: string[],
      proof: Groth16Proof
    ): Promise<boolean>;
  }

  export namespace zKey {
    function exportVerificationKey(zkeyFile: string): Promise<object>;
  }
}

declare module 'circomlibjs' {
  export function buildPoseidon(): Promise<{
    (inputs: (bigint | number | string)[]): Uint8Array;
    F: {
      toObject(hash: Uint8Array): bigint;
    };
  }>;

  export function buildEddsa(): Promise<{
    signPoseidon(privKey: Uint8Array, msg: bigint): {
      R8: [bigint, bigint];
      S: bigint;
    };
    verifyPoseidon(msg: bigint, sig: { R8: [bigint, bigint]; S: bigint }, pubKey: [bigint, bigint]): boolean;
    prv2pub(privKey: Uint8Array): [bigint, bigint];
  }>;

  export function buildBabyjub(): Promise<{
    F: {
      toObject(val: Uint8Array): bigint;
    };
    mulPointEscalar(base: [bigint, bigint], scalar: bigint): [bigint, bigint];
    Base8: [bigint, bigint];
  }>;
}

declare module 'ffjavascript' {
  export function getCurveFromName(name: string): Promise<object>;
  export const Scalar: {
    fromString(str: string, base?: number): bigint;
    toString(val: bigint, base?: number): string;
  };
}
