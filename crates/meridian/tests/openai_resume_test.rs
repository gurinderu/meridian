use meridian::openai::{openai_to_canonical, openai_to_canonical_resumable};
use serde_json::json;

#[test]
fn resume_omits_history_block_and_sends_delta() {
    let body = json!({"model":"sonnet","messages":[
        {"role":"user","content":"first"},
        {"role":"assistant","content":"reply one"},
        {"role":"user","content":"second"}]});
    // no-resume: history goes into system, prompt is the last user msg
    let (_m, sys_cold, prompt_cold) = openai_to_canonical(&body).unwrap();
    assert_eq!(prompt_cold, "second");
    assert!(sys_cold.as_deref().unwrap_or("").contains("conversation_history"));
    assert!(sys_cold.as_deref().unwrap_or("").contains("first"));
    // resume: no history block; prompt is still the delta (last user)
    let (_m2, sys_warm, prompt_warm) = openai_to_canonical_resumable(&body, true).unwrap();
    assert_eq!(prompt_warm, "second");
    assert!(!sys_warm.as_deref().unwrap_or("").contains("conversation_history"));
    // openai_to_canonical == resumable(false)
    let (_m3, sys_cold2, _p) = openai_to_canonical_resumable(&body, false).unwrap();
    assert_eq!(sys_cold2, sys_cold);
}
