use criterion::{
    criterion_group,
    criterion_main,
    measurement::WallTime,
    BenchmarkGroup,
    Criterion,
};
use fuel_core::service::{
    config::Trigger,
    Config,
    ServiceTrait,
};
use rand::SeedableRng;

use fuel_core_benches::*;
use fuel_core_types::{
    fuel_asm::{
        op,
        GTFArgs,
        Instruction,
        RegId,
    },
    fuel_tx::UniqueIdentifier,
    fuel_types::AssetId,
};

// Use Jemalloc during benchmarks
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

fn run(id: &str, group: &mut BenchmarkGroup<WallTime>, script: Vec<Instruction>) {
    group.bench_function(id, |b| {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _drop = rt.enter();
        const TARGET_BLOCK_GAS_LIMIT: u64 = 100_000;
        const BASE: u64 = 10_000;

        let database = Database::rocksdb();
        let mut config = Config::local_node();
        config.chain_conf.consensus_parameters.tx_params.max_gas_per_tx = TARGET_BLOCK_GAS_LIMIT;
        config
            .chain_conf
            .consensus_parameters
            .predicate_params
            .max_gas_per_predicate = TARGET_BLOCK_GAS_LIMIT;
        config.chain_conf.block_gas_limit = TARGET_BLOCK_GAS_LIMIT;
        config.utxo_validation = false;
        config.block_production = Trigger::Instant;

        let service = fuel_core::service::FuelService::new(database, config.clone())
            .expect("Unable to start a FuelService");
        service.start().expect("Unable to start the service");
        let mut rng = rand::rngs::StdRng::seed_from_u64(2322u64);

        b.to_async(&rt).iter(|| {
            let shared = service.shared.clone();
            let tx = fuel_core_types::fuel_tx::TransactionBuilder::script(
                // Infinite loop
                script.clone().into_iter().collect(),
                vec![],
            )
            .gas_limit(TARGET_BLOCK_GAS_LIMIT - BASE)
            .gas_price(1)
            .add_unsigned_coin_input(
                rng.gen(),
                rng.gen(),
                u64::MAX,
                AssetId::BASE,
                Default::default(),
                Default::default(),
            )
            .finalize_as_transaction();
            async move {
                let tx_id = tx.id(&config.chain_conf.consensus_parameters.chain_id);

                let mut sub = shared.block_importer.block_importer.subscribe();
                shared
                    .txpool
                    .insert(vec![std::sync::Arc::new(tx)])
                    .await
                    .into_iter()
                    .next()
                    .expect("Should be at least 1 element")
                    .expect("Should include transaction successfully");
                let res = sub.recv().await.expect("Should produce a block");
                assert_eq!(res.tx_status.len(), 2);
                assert_eq!(res.sealed_block.entity.transactions().len(), 2);
                assert_eq!(res.tx_status[1].id, tx_id);

                let fuel_core_types::services::executor::TransactionExecutionResult::Failed {
                    reason,
                    ..
                } = &res.tx_status[1].result
                    else {
                        panic!("The execution should fails with out of gas")
                    };
                assert!(reason.contains("OutOfGas"));
            }
        })
    });
}

fn block_target_gas(c: &mut Criterion) {
    let mut group = c.benchmark_group("block target estimation");

    run(
        "Script with noop opcode and infinite loop",
        &mut group,
        [op::noop(), op::jmpb(RegId::ZERO, 0)].to_vec(),
    );

    run(
        "Script with meq opcode and infinite loop",
        &mut group,
        [
            op::movi(0x10, (1 << 18) - 1),
            op::meq(0x11, RegId::SP, RegId::SP, 0x10),
            op::jmpb(RegId::ZERO, 0),
        ]
        .to_vec(),
    );

    run(
        "Script with logd opcode and infinite loop",
        &mut group,
        [
            op::movi(0x10, (1 << 18) - 1),
            op::logd(RegId::ZERO, RegId::ZERO, RegId::ZERO, 0x10),
            op::jmpb(RegId::ZERO, 0),
        ]
        .to_vec(),
    );

    run(
        "Script with gtf opcode and infinite loop",
        &mut group,
        [
            op::gtf(0x10, RegId::ZERO, GTFArgs::InputCoinOwner as u16),
            op::jmpb(RegId::ZERO, 0),
        ]
        .to_vec(),
    );

    // The test is supper long because we don't use `DependentCost` for k256 opcode
    // run(
    //     "Script with k256 opcode and infinite loop",
    //     &mut group,
    //     [
    //         op::movi(0x10, 1 << 18 - 1),
    //         op::aloc(0x10),
    //         op::k256(RegId::HP, RegId::ZERO, 0x10),
    //         op::jmpb(RegId::ZERO, 0),
    //     ]
    //     .to_vec(),
    // );

    group.finish();
}

criterion_group!(benches, block_target_gas);
criterion_main!(benches);
