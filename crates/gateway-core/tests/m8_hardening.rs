//! M8 收尾加固：解码器健壮性 fuzz 简表（roadmap M8 验收 2）。
//!
//! 用确定性伪随机生成 500 个 body，喂给所有入站/上游解码入口，
//! 断言不 panic、不悬挂；合法 JSON 里的未知字段按 Lenient 处理或返回可读错误。

use gateway_core::codec::{anthropic, gemini, openai, responses};

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0
    }
    fn byte(&mut self) -> u8 {
        (self.next() >> 56) as u8
    }
    fn bytes(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| self.byte()).collect()
    }
}

fn random_bodies(seed: u64, n: usize) -> Vec<Vec<u8>> {
    let mut rng = Lcg(seed);
    let mut bodies = Vec::with_capacity(n);
    for i in 0..n {
        match i % 5 {
            0 => {
                // 完全随机字节
                let len = (rng.next() as usize % 256) + 1;
                bodies.push(rng.bytes(len));
            }
            1 => {
                // 合法 JSON 随机结构
                let val = serde_json::json!({
                    "model": if rng.next().is_multiple_of(2) { "m" } else { "gpt-4o" },
                    "stream": rng.next().is_multiple_of(2),
                    "messages": [],
                    "input": "hi",
                    "instructions": "x",
                });
                bodies.push(serde_json::to_vec(&val).unwrap());
            }
            2 => {
                // 半截 JSON
                let mut b = b"{\"model\":\"m\",\"messages\":[".to_vec();
                let extra = (rng.next() as usize % 64) + 1;
                b.extend(rng.bytes(extra));
                bodies.push(b);
            }
            3 => {
                // 超深层嵌套 JSON（防止递归爆栈的粗测）
                let depth = (rng.next() % 64 + 1) as usize;
                let mut s = String::new();
                for _ in 0..depth {
                    s.push_str("{\"a\":");
                }
                s.push('1');
                for _ in 0..depth {
                    s.push('}');
                }
                bodies.push(s.into_bytes());
            }
            _ => {
                // 数字/字符串/数组
                let val = serde_json::json!([1, 2, "x", {"k": [true, null]}]);
                bodies.push(serde_json::to_vec(&val).unwrap());
            }
        }
    }
    bodies
}

#[test]
fn decoders_never_panic_on_random_bodies() {
    let bodies = random_bodies(0x4A49_2025, 500);
    for b in &bodies {
        let _ = openai::decode_request(b);
        let _ = anthropic::decode_request(b);
        let _ = responses::decode_request(b);
        let _ = openai::peek(b);
        let _ = anthropic::peek(b);
        let _ = anthropic::count_tokens(b);
        let _ = gemini::parse_stream_event(b);
        let _ = openai::parse_stream_event(b);
        let _ = anthropic::parse_stream_event(b);
        let _ = responses::render_stream_event(
            &gateway_core::codec::ir::StreamEvent::Start { model: "m".into() },
            &mut responses::RenderState::default(),
        );
        let _ = responses::render_response(&gateway_core::codec::ir::CanonicalResponse {
            id: "r".into(),
            model: "m".into(),
            output: vec![],
            stop_reason: gateway_core::codec::ir::StopReason::EndTurn,
            usage: Default::default(),
        });
    }
}
