use candid::{candid_method, Principal};
use ic_canisters_http_types::{HttpRequest, HttpResponse, HttpResponseBuilder};
use ic_cdk_macros::{init, post_upgrade, query, update};
use ic_icrc1::{blocks::icrc1_block_from_encoded, Block};
use ic_ledger_core::block::{BlockIndex, BlockType, EncodedBlock};
use ic_stable_structures::memory_manager::{MemoryId, VirtualMemory};
use ic_stable_structures::{
    cell::Cell as StableCell, log::Log as StableLog, memory_manager::MemoryManager,
    DefaultMemoryImpl, RestrictedMemory, Storable,
};
use icrc_ledger_types::block::{Block as IcrcBlock, BlockRange, GetBlocksArgs};
use icrc_ledger_types::transaction::{Transaction, TransactionRange};
use icrc_ledger_types::GetTransactionsRequest;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::cell::RefCell;

const WASM_PAGE_SIZE: u64 = 65536;

const GIB: u64 = 1024 * 1024 * 1024;

/// How much memory do we want to allocate for raw blocks.
const DEFAULT_MEMORY_LIMIT: u64 = 3 * GIB;

/// The maximum number of blocks to return in a single get_transactions request.
const DEFAULT_MAX_TRANSACTIONS_PER_GET_TRANSACTION_RESPONSE: u64 = 2000;

/// The maximum number of Wasm pages that we allow to use for the stable storage.
const NUM_WASM_PAGES: u64 = 4 * GIB / WASM_PAGE_SIZE;

const BLOCK_LOG_INDEX_MEMORY_ID: MemoryId = MemoryId::new(0);
const BLOCK_LOG_DATA_MEMORY_ID: MemoryId = MemoryId::new(1);

type Memory = RestrictedMemory<DefaultMemoryImpl>;
type BlockLog = StableLog<Vec<u8>, VirtualMemory<Memory>, VirtualMemory<Memory>>;
type ConfigCell = StableCell<ArchiveConfig, Memory>;

/// Creates a memory region for the configuration stable cell.
fn config_memory() -> Memory {
    RestrictedMemory::new(DefaultMemoryImpl::default(), 0..1)
}

/// Creates a memory region for the append-only block list.
fn blocks_memory() -> Memory {
    RestrictedMemory::new(DefaultMemoryImpl::default(), 1..NUM_WASM_PAGES)
}

thread_local! {
    /// Static configuration of the archive that init() sets once.
    static CONFIG: RefCell<ConfigCell> = RefCell::new(ConfigCell::init(
        config_memory(),
        ArchiveConfig::default(),
    ).expect("failed to initialize stable cell"));

    /// Static memory manager to manage the memory available for blocks.
    static MEMORY_MANAGER: RefCell<MemoryManager<Memory>> = RefCell::new(MemoryManager::init(blocks_memory()));

    /// Append-only list of encoded blocks stored in stable memory.
    static BLOCKS: RefCell<BlockLog> = with_memory_manager(|memory_manager| {
        RefCell::new(BlockLog::init(memory_manager.get(BLOCK_LOG_INDEX_MEMORY_ID), memory_manager.get(BLOCK_LOG_DATA_MEMORY_ID)).expect("failed to initialize stable log"))
    });
}

/// Configuration of the archive node.
#[derive(Serialize, Deserialize)]
struct ArchiveConfig {
    /// The maximum number of bytes archive can use to store encoded blocks.
    max_memory_size_bytes: u64,
    /// The index of the first block in the archive.
    block_index_offset: u64,
    /// The principal of the ledger canister that created this archive.
    /// The archive will accept blocks only from this principal.
    ledger_id: Principal,
    /// The maximum number of transactions returned by [get_transactions].
    max_transactions_per_response: u64,
}

// NOTE: the default configuration is dysfunctional, but it's convenient to have
// a Default impl for the initialization of the [CONFIG] variable above.
impl Default for ArchiveConfig {
    fn default() -> Self {
        Self {
            max_memory_size_bytes: 0,
            block_index_offset: 0,
            ledger_id: Principal::management_canister(),
            max_transactions_per_response: DEFAULT_MAX_TRANSACTIONS_PER_GET_TRANSACTION_RESPONSE,
        }
    }
}

impl Storable for ArchiveConfig {
    fn to_bytes(&self) -> Cow<[u8]> {
        let mut buf = vec![];
        ciborium::ser::into_writer(self, &mut buf).expect("failed to encode archive config");
        Cow::Owned(buf)
    }

    fn from_bytes(bytes: Cow<'_, [u8]>) -> Self {
        ciborium::de::from_reader(&bytes[..]).expect("failed to decode archive options")
    }
}

/// A helper function to access the configuration.
fn with_archive_opts<R>(f: impl FnOnce(&ArchiveConfig) -> R) -> R {
    CONFIG.with(|cell| f(cell.borrow().get()))
}

/// A helper function to access the memory manager.
fn with_memory_manager<R>(f: impl FnOnce(&MemoryManager<Memory>) -> R) -> R {
    MEMORY_MANAGER.with(|cell| f(&cell.borrow()))
}

/// A helper function to access the block list.
fn with_blocks<R>(f: impl FnOnce(&BlockLog) -> R) -> R {
    BLOCKS.with(|cell| f(&cell.borrow()))
}

fn decode_transaction(txid: u64, bytes: Vec<u8>) -> Transaction {
    Block::decode(EncodedBlock::from(bytes))
        .unwrap_or_else(|e| ic_cdk::api::trap(&format!("failed to decode block {}: {}", txid, e)))
        .into()
}

fn decode_icrc1_block(_txid: u64, bytes: Vec<u8>) -> IcrcBlock {
    let encoded_block = EncodedBlock::from(bytes);
    icrc1_block_from_encoded(&encoded_block)
}

#[init]
#[candid_method(init)]
fn init(
    ledger_id: Principal,
    block_index_offset: u64,
    max_memory_size_bytes: Option<u64>,
    max_transactions_per_response: Option<u64>,
) {
    CONFIG.with(|cell| {
        let max_memory_size_bytes = max_memory_size_bytes
            .unwrap_or(DEFAULT_MEMORY_LIMIT)
            .min(DEFAULT_MEMORY_LIMIT);
        let max_transactions_per_response = max_transactions_per_response
            .unwrap_or(DEFAULT_MAX_TRANSACTIONS_PER_GET_TRANSACTION_RESPONSE);
        cell.borrow_mut()
            .set(ArchiveConfig {
                max_memory_size_bytes,
                block_index_offset,
                ledger_id,
                max_transactions_per_response,
            })
            .expect("failed to set archive config");
    });

    MEMORY_MANAGER.with(|cell| *cell.borrow_mut() = MemoryManager::init(blocks_memory()));

    with_memory_manager(|memory_manager| {
        BLOCKS.with(|cell| {
            *cell.borrow_mut() = BlockLog::new(
                memory_manager.get(BLOCK_LOG_INDEX_MEMORY_ID),
                memory_manager.get(BLOCK_LOG_DATA_MEMORY_ID),
            )
        });
    })
}

#[post_upgrade]
fn post_upgrade() {
    // NB. we do not need to do anything to decode the values from the stable
    // memory: variable initializers take care of the decoding.  The only reason
    // we define the post_upgrade hook is to make sure that the first access to
    // stable variables happens in that hook.  This way the system will roll-back
    // the upgrade if the initialization traps.
    let max_memory_size_bytes = with_archive_opts(|opts| opts.max_memory_size_bytes);
    with_blocks(|blocks| assert!(blocks.log_size_bytes() <= max_memory_size_bytes));
}

#[update]
#[candid_method(update)]
fn append_blocks(new_blocks: Vec<EncodedBlock>) {
    let max_memory_size_bytes = with_archive_opts(|opts| {
        if ic_cdk::api::caller() != opts.ledger_id {
            ic_cdk::api::trap(&format!(
                "only {} can append blocks to this archive",
                opts.ledger_id
            ));
        }
        opts.max_memory_size_bytes
    });

    with_blocks(|blocks| {
        let bytes: u64 = new_blocks.iter().map(|b| b.size_bytes() as u64).sum();
        if max_memory_size_bytes < blocks.log_size_bytes().saturating_add(bytes) {
            ic_cdk::api::trap("no space left");
        }
        for block in new_blocks {
            blocks
                .append(&block.into_vec())
                .unwrap_or_else(|_| ic_cdk::api::trap("no space left"));
        }
    })
}

#[query]
#[candid_method(query)]
fn remaining_capacity() -> u64 {
    let total_block_size = with_blocks(|blocks| blocks.log_size_bytes());
    with_archive_opts(|opts| {
        opts.max_memory_size_bytes
            .checked_sub(total_block_size)
            .expect("bug: archive capacity underflow")
    })
}

#[query]
#[candid_method(query)]
fn get_transaction(index: BlockIndex) -> Option<Transaction> {
    let idx_offset = with_archive_opts(|opts| opts.block_index_offset);
    let relative_idx = (idx_offset <= index).then_some(index - idx_offset)?;

    let block = with_blocks(|blocks| blocks.get(relative_idx))?;
    Some(decode_transaction(index, block))
}

fn decode_block_range<R>(start: u64, length: u64, decoder: impl Fn(u64, Vec<u8>) -> R) -> Vec<R> {
    let offset = with_archive_opts(|opts| {
        if start < opts.block_index_offset {
            ic_cdk::api::trap(&format!(
                "requested index {} is less than the minimal index {} this archive serves",
                start, opts.block_index_offset
            ));
        }
        start - opts.block_index_offset
    });

    let length = length.min(with_archive_opts(|opts| opts.max_transactions_per_response));
    with_blocks(|blocks| {
        let limit = blocks.len().min(offset.saturating_add(length));
        (offset..limit)
            .map(|i| decoder(start + i, blocks.get(i).unwrap()))
            .collect()
    })
}

#[query]
#[candid_method(query)]
fn get_transactions(req: GetTransactionsRequest) -> TransactionRange {
    let (start, length) = req
        .as_start_and_length()
        .unwrap_or_else(|msg| ic_cdk::api::trap(&msg));

    let transactions = decode_block_range(start, length, decode_transaction);
    TransactionRange { transactions }
}

/// Get length Blocks starting at start BlockIndex.
#[query]
#[candid_method(query)]
fn get_blocks(req: GetBlocksArgs) -> BlockRange {
    let (start, length) = req
        .as_start_and_length()
        .unwrap_or_else(|msg| ic_cdk::api::trap(&msg));

    let blocks = decode_block_range(start, length, decode_icrc1_block);
    BlockRange { blocks }
}

#[query]
fn __get_candid_interface_tmp_hack() -> &'static str {
    include_str!(env!("ARCHIVE_DID_PATH"))
}

fn encode_metrics(w: &mut ic_metrics_encoder::MetricsEncoder<Vec<u8>>) -> std::io::Result<()> {
    w.encode_gauge(
        "archive_stable_memory_pages",
        ic_cdk::api::stable::stable_size() as f64,
        "Size of the stable memory allocated by this canister measured in 64K Wasm pages.",
    )?;
    w.encode_gauge(
        "archive_stable_memory_bytes",
        ic_cdk::api::stable::stable_size() as f64 * 65536f64,
        "Size of the stable memory allocated by this canister.",
    )?;
    w.encode_gauge(
        "archive_cycle_balance",
        ic_cdk::api::canister_balance128() as f64,
        "Cycle balance on this canister.",
    )?;
    w.encode_gauge(
        "archive_stored_blocks",
        with_blocks(|blocks| blocks.len()) as f64,
        "Total number of blocks stored in the main memory.",
    )?;

    Ok(())
}

#[candid_method(query)]
#[query]
fn http_request(req: HttpRequest) -> HttpResponse {
    if req.path() == "/metrics" {
        let mut writer =
            ic_metrics_encoder::MetricsEncoder::new(vec![], ic_cdk::api::time() as i64 / 1_000_000);

        match encode_metrics(&mut writer) {
            Ok(()) => HttpResponseBuilder::ok()
                .header("Content-Type", "text/plain; version=0.0.4")
                .with_body_and_content_length(writer.into_inner())
                .build(),
            Err(err) => {
                HttpResponseBuilder::server_error(format!("Failed to encode metrics: {}", err))
                    .build()
            }
        }
    } else {
        HttpResponseBuilder::not_found().build()
    }
}

fn main() {}

#[test]
fn check_candid_interface() {
    use candid::utils::{service_compatible, CandidSource};
    use std::path::PathBuf;

    candid::export_service!();

    let new_interface = __export_service();

    // check the public interface against the actual one
    let old_interface =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("archive.did");

    service_compatible(
        CandidSource::Text(&new_interface),
        CandidSource::File(old_interface.as_path()),
    )
    .expect("the ledger interface is not compatible with archive.did");
}
