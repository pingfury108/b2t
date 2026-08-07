use serde::Serialize;

#[derive(Serialize, Clone)]
pub struct Voice {
    pub id: &'static str,
    pub name: &'static str,
}

const VOICES: &[Voice] = &[
    Voice { id: "zh-CN-XiaoxiaoNeural", name: "晓晓 · 女声（温暖）" },
    Voice { id: "zh-CN-XiaoyiNeural", name: "晓伊 · 女声（活泼）" },
    Voice { id: "zh-CN-XiaohanNeural", name: "晓涵 · 女声（知性）" },
    Voice { id: "zh-CN-XiaomengNeural", name: "晓梦 · 女声（甜美）" },
    Voice { id: "zh-CN-XiaomoNeural", name: "晓墨 · 女声（沉稳）" },
    Voice { id: "zh-CN-XiaoxuanNeural", name: "晓萱 · 女声（柔和）" },
    Voice { id: "zh-CN-XiaoyanNeural", name: "晓颜 · 女声（清亮）" },
    Voice { id: "zh-CN-XiaorouNeural", name: "晓柔 · 女声（轻柔）" },
    Voice { id: "zh-CN-XiaozhenNeural", name: "晓甄 · 女声（端庄）" },
    Voice { id: "zh-CN-YunxiNeural", name: "云希 · 男声（清朗）" },
    Voice { id: "zh-CN-YunyangNeural", name: "云扬 · 男声（专业）" },
    Voice { id: "zh-CN-YunjianNeural", name: "云健 · 男声（浑厚）" },
    Voice { id: "zh-CN-YunfengNeural", name: "云枫 · 男声（磁性）" },
    Voice { id: "zh-CN-YunhaoNeural", name: "云皓 · 男声（阳光）" },
    Voice { id: "zh-CN-YunzeNeural", name: "云泽 · 男声（成熟）" },
    Voice { id: "zh-CN-XiaochenNeural", name: "晓辰 · 女声（慵懒）" },
    Voice { id: "en-US-AriaNeural", name: "Aria · English Female" },
    Voice { id: "en-US-GuyNeural", name: "Guy · English Male" },
    Voice { id: "en-US-JennyNeural", name: "Jenny · English Female" },
    Voice { id: "en-US-AndrewNeural", name: "Andrew · English Male" },
];

pub fn voices() -> &'static [Voice] {
    VOICES
}
