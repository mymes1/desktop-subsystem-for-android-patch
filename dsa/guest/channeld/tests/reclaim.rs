//! When the daemon asks init to drop the guest's caches so free page reporting can hand the
//! memory back to the host.

use std::sync::mpsc;
use std::time::Duration;

use dsa_guest_channeld::reclaim::{MemInfo, Reason, ReclaimPolicy, Reclaimer, RECLAIM_PROPERTY};
use dsa_guest_channeld::router::{Peer, Router};
use dsa_protocol::control::envelope::Body;
use dsa_protocol::control::{Envelope, HostMemory, ScreenState, TaskRemoved, TaskUpdated};

const MIB: u64 = 1024;

/// A guest of 4 GiB with `free` MiB free and `cached` MiB of page cache.
fn memory(free: u64, cached: u64) -> MemInfo {
    MemInfo {
        mem_total_kb: 4096 * MIB,
        mem_free_kb: free * MIB,
        buffers_kb: 0,
        cached_kb: cached * MIB,
        shmem_kb: 0,
        slab_reclaimable_kb: 0,
        anon_pages_kb: 0,
    }
}

fn secs(s: u64) -> Duration {
    Duration::from_secs(s)
}

#[test]
fn nfr2_meminfo_is_read_in_kib_and_shared_memory_is_not_droppable() {
    let info = MemInfo::parse(
        "MemTotal:        4015356 kB\nMemFree:          812444 kB\nBuffers:           10240 kB\n\
         Cached:          1503112 kB\nShmem:             20480 kB\nSReclaimable:      80000 kB\n\
         AnonPages:        700000 kB\n\
         HugePages_Total:       0\nnonsense\n",
    );
    assert_eq!(info.mem_total_kb, 4015356);
    assert_eq!(info.mem_free_kb, 812444);
    assert_eq!(info.droppable_kb(), 1503112 - 20480 + 10240 + 80000);
    assert_eq!(info.used_kb(), 4015356 - 812444);
    assert_eq!(info.anon_pages_kb, 700000);
    assert_eq!(info.reclaimable_kb(), info.droppable_kb() + 700000);
}

#[test]
fn nfr2_an_idle_guest_drops_its_caches_once_after_the_idle_delay() {
    let mut reclaimer = Reclaimer::new(ReclaimPolicy::default());
    let mostly_free = memory(3000, 400);
    assert_eq!(reclaimer.decide(secs(0), 0, &mostly_free), None);
    assert_eq!(reclaimer.decide(secs(30), 0, &mostly_free), None);
    assert_eq!(
        reclaimer.decide(secs(60), 0, &mostly_free),
        Some(Reason::Idle)
    );
    // Once per idle stretch, and the cache is no longer most of what is in use.
    assert_eq!(reclaimer.decide(secs(90), 0, &memory(3500, 200)), None);
}

#[test]
fn nfr2_opening_a_task_starts_a_new_idle_stretch() {
    let mut reclaimer = Reclaimer::new(ReclaimPolicy::default());
    let small_cache = memory(2000, 300);
    reclaimer.decide(secs(0), 0, &small_cache);
    assert_eq!(
        reclaimer.decide(secs(60), 0, &small_cache),
        Some(Reason::Idle)
    );
    assert_eq!(reclaimer.decide(secs(90), 2, &small_cache), None);
    assert_eq!(reclaimer.decide(secs(120), 0, &small_cache), None);
    // Idle again, a full idle delay after the last task closed, even inside the gap.
    assert_eq!(
        reclaimer.decide(secs(180), 0, &small_cache),
        Some(Reason::Idle)
    );
}

#[test]
fn nfr2_with_tasks_open_caches_are_dropped_only_when_they_are_most_of_memory() {
    let mut reclaimer = Reclaimer::new(ReclaimPolicy::default());
    // 1096 MiB in use, 300 MiB of it cache: not most of it.
    assert_eq!(reclaimer.decide(secs(0), 1, &memory(3000, 300)), None);
    // 2096 MiB in use, 1500 MiB of it cache.
    assert_eq!(
        reclaimer.decide(secs(30), 1, &memory(2000, 1500)),
        Some(Reason::MostlyCache)
    );
    // Not again before the gap has passed.
    assert_eq!(reclaimer.decide(secs(60), 1, &memory(2000, 1500)), None);
    let gap = ReclaimPolicy::default().min_gap.as_secs();
    assert_eq!(
        reclaimer.decide(secs(30 + gap), 1, &memory(2000, 1500)),
        Some(Reason::MostlyCache)
    );
}

#[test]
fn nfr2_a_little_cache_is_not_worth_dropping() {
    let mut reclaimer = Reclaimer::new(ReclaimPolicy::default());
    let tiny = memory(4000, 64);
    reclaimer.decide(secs(0), 0, &tiny);
    assert_eq!(reclaimer.decide(secs(120), 0, &tiny), None);
}

/// An idle guest whose cache is gone, holding `anon` MiB of anonymous memory.
fn anon_only(anon: u64) -> MemInfo {
    MemInfo {
        anon_pages_kb: anon * MIB,
        ..memory(3000, 0)
    }
}

#[test]
fn nfr1_anonymous_memory_alone_is_worth_an_idle_reclaim_but_not_a_busy_one() {
    let mut reclaimer = Reclaimer::new(ReclaimPolicy::default());
    reclaimer.decide(secs(0), 0, &anon_only(700));
    assert_eq!(
        reclaimer.decide(secs(60), 0, &anon_only(700)),
        Some(Reason::Idle)
    );
    let mut reclaimer = Reclaimer::new(ReclaimPolicy::default());
    assert_eq!(reclaimer.decide(secs(0), 1, &anon_only(700)), None);
}

#[test]
fn nfr1_an_idle_guest_reclaims_again_only_when_memory_has_grown_back() {
    let mut reclaimer = Reclaimer::new(ReclaimPolicy::default());
    reclaimer.decide(secs(0), 0, &anon_only(900));
    assert_eq!(
        reclaimer.decide(secs(60), 0, &anon_only(900)),
        Some(Reason::Idle)
    );
    // What the reclaim left is the floor.
    assert_eq!(reclaimer.decide(secs(90), 0, &anon_only(300)), None);
    // Grown back, but not yet time.
    assert_eq!(reclaimer.decide(secs(120), 0, &anon_only(500)), None);
    // Time, but it stayed small.
    assert_eq!(reclaimer.decide(secs(360), 0, &anon_only(350)), None);
    // Time, and grown back: this is also how a reclaim made during boot gets its second go.
    assert_eq!(
        reclaimer.decide(secs(390), 0, &anon_only(500)),
        Some(Reason::Idle)
    );
}

#[test]
fn nfr1_minimized_tasks_do_not_hold_the_idle_reclaim_back() {
    let (to_host, _host) = mpsc::channel();
    let router = Router::new(to_host, |_| Err("no agent here".to_string()));
    router
        .from_guest(
            Peer(10),
            frame(Body::TaskUpdated(TaskUpdated {
                task_id: 1,
                package_name: "org.example.app".to_string(),
                ..Default::default()
            })),
        )
        .unwrap();
    assert_eq!(router.tasks_in_use(), 1);
    router.from_host(&frame(Body::ScreenState(ScreenState { on: false })));
    assert_eq!(router.tasks_in_use(), 0);
    assert_eq!(router.task_count(), 1);
    router.from_host(&frame(Body::ScreenState(ScreenState { on: true })));
    assert_eq!(router.tasks_in_use(), 1);
    // A host that connects again takes the screen to be on, and says so if it is not.
    router.from_host(&frame(Body::ScreenState(ScreenState { on: false })));
    router.host_connected();
    assert_eq!(router.tasks_in_use(), 1);
}

#[test]
fn nfr2_init_is_asked_through_a_vendor_property() {
    // Only vendor properties may be given a type in the image's vendor policy.
    assert!(RECLAIM_PROPERTY.starts_with("vendor."));
}

fn frame(body: Body) -> Vec<u8> {
    dsa_protocol::encode(&Envelope { body: Some(body) })
}

#[test]
fn nfr2_the_router_counts_the_open_tasks_the_reclaim_waits_for() {
    let (to_host, _host) = mpsc::channel();
    let router = Router::new(to_host, |_| Err("no agent here".to_string()));
    let agent = Peer(10);
    assert_eq!(router.task_count(), 0);
    for task_id in [1, 2] {
        router
            .from_guest(
                agent,
                frame(Body::TaskUpdated(TaskUpdated {
                    task_id,
                    package_name: format!("org.example.app{task_id}"),
                    ..Default::default()
                })),
            )
            .unwrap();
    }
    assert_eq!(router.task_count(), 2);
    router
        .from_guest(agent, frame(Body::TaskRemoved(TaskRemoved { task_id: 1 })))
        .unwrap();
    assert_eq!(router.task_count(), 1);
}

#[test]
fn nfr2_open_tasks_get_a_gradual_trim_of_cold_cache_every_so_often() {
    use dsa_guest_channeld::reclaim::TRIM_PROPERTY;
    assert!(TRIM_PROPERTY.starts_with("vendor."));
    let mut reclaimer = Reclaimer::new(ReclaimPolicy::default());
    // 1.4 GiB used, 600 MiB of it cache: mostly not cache, but 344 MiB above what is kept.
    let busy = memory(2600, 600);
    assert_eq!(
        reclaimer.decide_under(secs(30), 2, &busy, false),
        Some(Reason::Gradual)
    );
    // Not again before the gap.
    assert_eq!(reclaimer.decide_under(secs(60), 2, &busy, false), None);
    assert_eq!(
        reclaimer.decide_under(secs(75), 2, &busy, false),
        Some(Reason::Gradual)
    );
}

#[test]
fn nfr2_the_gradual_trim_leaves_the_cache_apps_use_and_waits_out_pressure() {
    let mut reclaimer = Reclaimer::new(ReclaimPolicy::default());
    // 300 MiB of cache is under what the trim keeps plus its least slice.
    assert_eq!(
        reclaimer.decide_under(secs(30), 2, &memory(2600, 300), false),
        None
    );
    // Under memory pressure nothing is trimmed, however much cache there is.
    assert_eq!(
        reclaimer.decide_under(secs(60), 2, &memory(1500, 900), true),
        None
    );
    // No tasks: the idle reclaim owns that case, not the trim.
    assert_eq!(
        reclaimer.decide_under(secs(90), 0, &memory(1500, 900), false),
        None
    );
}

#[test]
fn nfr2_memory_pressure_is_read_from_the_some_line_and_unreadable_counts_as_busy() {
    use dsa_guest_channeld::reclaim::memory_pressure;
    let calm = "some avg10=0.50 avg60=0.20 avg300=0.10 total=1234\nfull avg10=0.00 avg60=0.00 avg300=0.00 total=0\n";
    let busy = "some avg10=7.25 avg60=1.00 avg300=0.10 total=1234\nfull avg10=0.00 avg60=0.00 avg300=0.00 total=0\n";
    assert!(!memory_pressure(calm, 5.0));
    assert!(memory_pressure(busy, 5.0));
    assert!(memory_pressure("", 5.0));
}

#[test]
fn nfr2_a_host_short_of_memory_gets_it_back_with_tasks_open_and_again_while_short() {
    let mut reclaimer = Reclaimer::new(ReclaimPolicy::default());
    let cache = memory(2000, 600);
    // Tasks are open and cache is not most of memory: nothing is due on the guest's account.
    assert_eq!(reclaimer.decide_under(secs(0), 2, &cache, true), None);
    reclaimer.host_short(true);
    assert_eq!(
        reclaimer.decide_under(secs(30), 2, &cache, true),
        Some(Reason::HostShort)
    );
    // Not again at once, but again after the gap while the host stays short.
    assert_eq!(reclaimer.decide_under(secs(60), 2, &cache, true), None);
    assert_eq!(
        reclaimer.decide_under(secs(90), 2, &cache, true),
        Some(Reason::HostShort)
    );
    // Once the host is no longer short, the guest keeps what it has.
    reclaimer.host_short(false);
    assert_eq!(reclaimer.decide_under(secs(200), 2, &cache, true), None);
}

#[test]
fn nfr2_the_router_remembers_whether_the_host_is_short_of_memory() {
    let (to_host, _host) = mpsc::channel();
    let router = Router::new(to_host, |_| Err("no agent here".to_string()));
    assert!(!router.host_memory_low());
    router.from_host(&frame(Body::HostMemory(HostMemory { low: true })));
    assert!(router.host_memory_low());
    router.from_host(&frame(Body::HostMemory(HostMemory { low: false })));
    assert!(!router.host_memory_low());
    // A host that connects again says so again if it still is.
    router.from_host(&frame(Body::HostMemory(HostMemory { low: true })));
    router.host_connected();
    assert!(!router.host_memory_low());
}

#[test]
fn nfr1_cache_beyond_the_working_set_goes_back_without_host_pressure_in_bounded_steps() {
    use dsa_guest_channeld::reclaim::TRIM_SIZE_PROPERTY;
    assert!(TRIM_SIZE_PROPERTY.starts_with("vendor."));
    let mut reclaimer = Reclaimer::new(ReclaimPolicy::default());
    reclaimer.host_short(false);
    // One app open, 1100 MiB of page cache, the host not short: the cache is trimmed anyway,
    // at most 128 MiB at a time.
    let busy = memory(1200, 1100);
    assert_eq!(
        reclaimer.decide_under(secs(0), 1, &busy, false),
        Some(Reason::Gradual)
    );
    assert_eq!(reclaimer.trim_mib(), 128);
    assert_eq!(reclaimer.decide_under(secs(30), 1, &busy, false), None);
    assert_eq!(
        reclaimer.decide_under(secs(45), 1, &busy, false),
        Some(Reason::Gradual)
    );
    // Just above the target: only the excess, in whole MiB.
    assert_eq!(
        reclaimer.decide_under(secs(90), 1, &memory(3000, 256 + 80), false),
        Some(Reason::Gradual)
    );
    assert_eq!(reclaimer.trim_mib(), 80);
    // At the target: nothing.
    assert_eq!(
        reclaimer.decide_under(secs(135), 1, &memory(3000, 300), false),
        None
    );
}

#[test]
fn nfr1_an_idle_guest_keeps_its_cache_at_the_working_set_between_full_reclaims() {
    let mut reclaimer = Reclaimer::new(ReclaimPolicy::default());
    reclaimer.decide_under(secs(0), 0, &memory(3000, 400), false);
    assert_eq!(
        reclaimer.decide_under(secs(60), 0, &memory(3000, 400), false),
        Some(Reason::Idle)
    );
    // Background work reads files again long before the next full reclaim is due.
    assert_eq!(
        reclaimer.decide_under(secs(120), 0, &memory(3000, 600), false),
        Some(Reason::Gradual)
    );
}

#[test]
fn nfr1_a_trim_waits_until_the_open_tasks_have_settled() {
    let mut reclaimer = Reclaimer::new(ReclaimPolicy::default());
    let cache = memory(2600, 600);
    assert_eq!(
        reclaimer.decide_under(secs(0), 0, &memory(3500, 100), false),
        None
    );
    // An app opens: it is reading its files, so nothing is trimmed yet.
    assert_eq!(reclaimer.decide_under(secs(10), 1, &cache, false), None);
    assert_eq!(reclaimer.decide_under(secs(30), 1, &cache, false), None);
    assert_eq!(
        reclaimer.decide_under(secs(40), 1, &cache, false),
        Some(Reason::Gradual)
    );
}

#[test]
fn nfr1_file_refaults_raise_the_working_set_target_and_hold_the_trim() {
    let mut reclaimer = Reclaimer::new(ReclaimPolicy::default());
    let cache = memory(2600, 600);
    reclaimer.refaults(0);
    assert_eq!(reclaimer.working_set_target_kb(), 256 * MIB);
    // 40 MiB of evicted cache came back: an app still uses it.
    reclaimer.refaults(40 * MIB);
    assert_eq!(reclaimer.working_set_target_kb(), 320 * MIB);
    assert_eq!(reclaimer.decide_under(secs(0), 1, &cache, false), None);
    // Quiet again: the target falls back and the trim resumes.
    reclaimer.refaults(41 * MIB);
    assert_eq!(reclaimer.working_set_target_kb(), 256 * MIB);
    assert_eq!(
        reclaimer.decide_under(secs(30), 1, &cache, false),
        Some(Reason::Gradual)
    );
}

#[test]
fn nfr1_quiet_refaults_let_the_target_fall_back_but_never_below_its_floor() {
    let mut reclaimer = Reclaimer::new(ReclaimPolicy::default());
    reclaimer.refaults(0);
    for step in 1..=20 {
        reclaimer.refaults(step * 20 * MIB);
    }
    assert_eq!(reclaimer.working_set_target_kb(), 1024 * MIB);
    for _ in 0..30 {
        reclaimer.refaults(400 * MIB);
    }
    assert_eq!(reclaimer.working_set_target_kb(), 256 * MIB);
}

#[test]
fn nfr1_vmstat_file_refaults_are_read_in_kib() {
    use dsa_guest_channeld::reclaim::file_refaults_kb;
    let vmstat = "nr_free_pages 1000\nworkingset_refault_anon 7\nworkingset_refault_file 2560\n";
    assert_eq!(file_refaults_kb(vmstat, 4), Some(10240));
    assert_eq!(file_refaults_kb("nr_free_pages 1\n", 4), None);
}
