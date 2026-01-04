pragma circom 2.1.6;

include "circomlib/circuits/poseidon.circom";
include "circomlib/circuits/bitify.circom";
include "circomlib/circuits/comparators.circom";
include "circomlib/circuits/eddsaposeidon.circom";
include "circomlib/circuits/pedersen.circom";

template RinkuPrivateProof(merkleDepth) {
    signal input txHash;
    signal input senderPrivKey;
    signal input senderPubKeyX;
    signal input senderPubKeyY;
    signal input txSigR8X;
    signal input txSigR8Y;
    signal input txSigS;
    
    signal input merklePathElements[merkleDepth];
    signal input merklePathIndices[merkleDepth];
    
    signal input amount;
    signal input amountBlinding;
    
    signal input checkpointHeight;
    signal input chainId;
    
    signal output checkpointRoot;
    signal output nullifier;
    signal output amountCommitment;
    signal output chainIdHash;

    component sigVerifier = EdDSAPoseidonVerifier();
    sigVerifier.enabled <== 1;
    sigVerifier.Ax <== senderPubKeyX;
    sigVerifier.Ay <== senderPubKeyY;
    sigVerifier.S <== txSigS;
    sigVerifier.R8x <== txSigR8X;
    sigVerifier.R8y <== txSigR8Y;
    sigVerifier.M <== txHash;

    component merkleHashers[merkleDepth];
    signal intermediates[merkleDepth + 1];
    intermediates[0] <== txHash;

    for (var i = 0; i < merkleDepth; i++) {
        merkleHashers[i] = Poseidon(2);
        
        merkleHashers[i].inputs[0] <== intermediates[i] + (merklePathElements[i] - intermediates[i]) * merklePathIndices[i];
        merkleHashers[i].inputs[1] <== merklePathElements[i] + (intermediates[i] - merklePathElements[i]) * merklePathIndices[i];
        
        intermediates[i + 1] <== merkleHashers[i].out;
    }
    
    checkpointRoot <== intermediates[merkleDepth];

    component nullifierHasher = Poseidon(3);
    nullifierHasher.inputs[0] <== senderPrivKey;
    nullifierHasher.inputs[1] <== checkpointHeight;
    nullifierHasher.inputs[2] <== txHash;
    nullifier <== nullifierHasher.out;

    component amountCommitter = Poseidon(2);
    amountCommitter.inputs[0] <== amount;
    amountCommitter.inputs[1] <== amountBlinding;
    amountCommitment <== amountCommitter.out;

    component chainIdHasher = Poseidon(1);
    chainIdHasher.inputs[0] <== chainId;
    chainIdHash <== chainIdHasher.out;
}

component main = RinkuPrivateProof(10);
