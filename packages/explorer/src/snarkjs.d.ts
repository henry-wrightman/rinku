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
