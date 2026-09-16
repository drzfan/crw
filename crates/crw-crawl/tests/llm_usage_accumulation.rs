//! Every leg of a multi-leg request must contribute its tokens.
//!
//! The SaaS bills straight off `llm_usage`, so a leg whose tokens are dropped
//! is work nobody pays for. Four call sites used to write
//! `if data.llm_usage.is_none() { data.llm_usage = result.usage }`, which keeps
//! whichever leg landed first and silently discards every later one. A request
//! asking for `json` and `summary` together runs two models; change tracking in
//! json mode adds a third.
//!
//! `LlmUsage::merge` (crw-core) is the documented way to combine legs. These
//! tests pin the semantics the call sites rely on, and the source guard below
//! pins that the call sites actually use it.

use crw_core::types::LlmUsage;

fn leg(input: u32, output: u32) -> LlmUsage {
    LlmUsage {
        input_tokens: input,
        output_tokens: output,
        total_tokens: input + output,
        estimated_cost_usd: None,
        model: "test-model".to_string(),
        provider: "test-provider".to_string(),
        cache_hit_input_tokens: None,
        cache_miss_input_tokens: None,
        truncated: false,
        calls: 1,
        executed_summaries: 0,
        answer_executed: false,
    }
}

#[test]
fn accumulate_sums_every_leg_rather_than_keeping_the_first() {
    let mut slot = None;
    LlmUsage::accumulate(&mut slot, Some(leg(600, 40)));
    LlmUsage::accumulate(&mut slot, Some(leg(900, 80)));
    LlmUsage::accumulate(&mut slot, Some(leg(300, 20)));

    let got = slot.expect("three legs ran, so usage must be present");
    assert_eq!(got.input_tokens, 1800, "input of all three legs");
    assert_eq!(got.output_tokens, 140, "output of all three legs");
    assert_eq!(got.total_tokens, 1940);
    assert_eq!(got.calls, 3, "one call per leg");
}

#[test]
fn accumulate_into_an_occupied_slot_does_not_discard_the_new_leg() {
    // This is the exact shape of the bug: the slot is already Some, because an
    // earlier format ran. The overwrite-if-empty guard dropped this leg.
    let mut slot = Some(leg(600, 40));
    LlmUsage::accumulate(&mut slot, Some(leg(900, 80)));

    let got = slot.unwrap();
    assert_eq!(got.input_tokens, 1500);
    assert_eq!(got.output_tokens, 120);
    assert_eq!(got.calls, 2);
}

#[test]
fn accumulate_is_a_no_op_when_a_leg_reported_nothing() {
    let mut slot = Some(leg(600, 40));
    LlmUsage::accumulate(&mut slot, None);
    assert_eq!(slot.unwrap().input_tokens, 600);

    let mut empty = None;
    LlmUsage::accumulate(&mut empty, None);
    assert!(empty.is_none(), "no legs ran, so no usage is reported");
}

/// Source guard: no call site may go back to overwrite-if-empty.
///
/// There is no type-level way to forbid it (`llm_usage` is a public field on a
/// public struct), and the bug is invisible in review because the line reads
/// like ordinary defensive code. It cost a customer real money on the SaaS
/// side, so it gets a test rather than a comment.
#[test]
fn no_call_site_overwrites_llm_usage_only_when_empty() {
    let roots = [
        concat!(env!("CARGO_MANIFEST_DIR"), "/src"),
        concat!(env!("CARGO_MANIFEST_DIR"), "/../crw-extract/src"),
    ];
    let mut offenders = Vec::new();

    for root in roots {
        let mut stack = vec![std::path::PathBuf::from(root)];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let lines: Vec<&str> = text.lines().collect();
                for (i, line) in lines.iter().enumerate() {
                    if !line.contains("llm_usage.is_none()") {
                        continue;
                    }
                    // An assertion in a test is fine; an `if` guard that then
                    // assigns is the bug.
                    if line.trim_start().starts_with("assert") {
                        continue;
                    }
                    let assigns = lines[i..(i + 4).min(lines.len())]
                        .iter()
                        .any(|l| l.contains("llm_usage =") && !l.contains("=="));
                    if assigns {
                        offenders.push(format!("{}:{}", path.display(), i + 1));
                    }
                }
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "these sites overwrite `llm_usage` only when empty, which discards \
         every leg after the first. Use `LlmUsage::accumulate` instead: {offenders:#?}"
    );
}

// ── Cache accounting across legs ─────────────────────────────────────────────
//
// The biller prefers an explicit cache-miss count over deriving `input - hit`,
// so a leg that reports no cache fields must not be folded in as if it consumed
// nothing. It did consume its input, and by definition none of it came from
// cache. Dormant while a provider reports no cache fields at all (every leg is
// `None`, and the biller derives); live the moment one leg reports and another
// does not, which is what a mixed structured + summary response looks like.

fn leg_with_cache(input: u32, output: u32, hit: Option<u32>, miss: Option<u32>) -> LlmUsage {
    LlmUsage {
        input_tokens: input,
        output_tokens: output,
        total_tokens: input + output,
        estimated_cost_usd: None,
        model: "test-model".to_string(),
        provider: "test-provider".to_string(),
        cache_hit_input_tokens: hit,
        cache_miss_input_tokens: miss,
        truncated: false,
        calls: 1,
        executed_summaries: 0,
        answer_executed: false,
    }
}

#[test]
fn a_leg_without_cache_counts_still_contributes_its_input_as_a_miss() {
    // json leg reports cache, summary leg does not.
    let mut slot = None;
    LlmUsage::accumulate(
        &mut slot,
        Some(leg_with_cache(4500, 50, Some(500), Some(4000))),
    );
    LlmUsage::accumulate(&mut slot, Some(leg_with_cache(6000, 80, None, None)));

    let got = slot.unwrap();
    assert_eq!(got.input_tokens, 10500);
    assert_eq!(got.cache_hit_input_tokens, Some(500));
    assert_eq!(
        got.cache_miss_input_tokens,
        Some(10000),
        "4000 reported by the first leg plus the second leg's whole 6000 input, \
         which was served from no cache. Carrying Some(4000) forward would bill \
         6000 tokens at zero."
    );
    // The parts must still add up to the whole.
    assert_eq!(
        got.cache_hit_input_tokens.unwrap() + got.cache_miss_input_tokens.unwrap(),
        got.input_tokens
    );
}

#[test]
fn the_reporting_leg_can_arrive_second_too() {
    let mut slot = None;
    LlmUsage::accumulate(&mut slot, Some(leg_with_cache(6000, 80, None, None)));
    LlmUsage::accumulate(
        &mut slot,
        Some(leg_with_cache(4500, 50, Some(500), Some(4000))),
    );

    let got = slot.unwrap();
    assert_eq!(got.cache_miss_input_tokens, Some(10000));
    assert_eq!(
        got.cache_hit_input_tokens.unwrap() + got.cache_miss_input_tokens.unwrap(),
        got.input_tokens
    );
}

#[test]
fn all_legs_silent_on_cache_stays_none_so_the_biller_derives() {
    let mut slot = None;
    LlmUsage::accumulate(&mut slot, Some(leg_with_cache(4000, 40, None, None)));
    LlmUsage::accumulate(&mut slot, Some(leg_with_cache(3000, 30, None, None)));

    let got = slot.unwrap();
    assert_eq!(got.input_tokens, 7000);
    assert_eq!(got.cache_hit_input_tokens, None);
    assert_eq!(
        got.cache_miss_input_tokens, None,
        "no leg reported cache, so leave it unset and let the biller derive it"
    );
}
