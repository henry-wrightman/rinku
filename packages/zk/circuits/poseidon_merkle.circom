pragma circom 2.1.6;

include "node_modules/circomlib/circuits/poseidon.circom";
include "node_modules/circomlib/circuits/bitify.circom";
include "node_modules/circomlib/circuits/comparators.circom";

template PoseidonMerkleProof(levels) {
    signal input leaf;
    signal input pathElements[levels];
    signal input pathIndices[levels];
    signal output root;

    component hashers[levels];
    component indexBits[levels];

    signal intermediates[levels + 1];
    intermediates[0] <== leaf;

    for (var i = 0; i < levels; i++) {
        indexBits[i] = Num2Bits(1);
        indexBits[i].in <== pathIndices[i];

        hashers[i] = Poseidon(2);
        
        hashers[i].inputs[0] <== intermediates[i] + (pathElements[i] - intermediates[i]) * pathIndices[i];
        hashers[i].inputs[1] <== pathElements[i] + (intermediates[i] - pathElements[i]) * pathIndices[i];
        
        intermediates[i + 1] <== hashers[i].out;
    }

    root <== intermediates[levels];
}

component main {public [root]} = PoseidonMerkleProof(10);
