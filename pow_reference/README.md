# Pow Reference 

## Overview - Just for reference

These code files and snippets are for reference - to see the differences between PoS and PoW. They are from the old (now removed) tutorial how to implement PoW. 

## What this code does

The code is really just a partial PoW implementation, it's missing the "work" part where work is done to find blocks. So it's just really showing how to use the substrate primitive and client packages for PoW - sc_consensus_pow and sp_consensus_pow.

It's outdated in that it doesn't work out of the box with the latest versions of these pallets. 

## Where it came from

Fork of the original examples code repo, preserved by forks on GH. 
https://github.com/n13/substrate-how-to-guides/tree/main/example-code/consensus-nodes/POW

## Packages

These fit with the package dependencies in workspace

sp-consensus-pow v0.40.0
https://crates.io/crates/sp-consensus-pow/0.40.0/dependencies

sc-consensis pow 0.44.0
https://crates.io/crates/sc-consensus-pow/0.44.0/dependencies

