//! Thin PDH wrapper (English counter paths, so it works on any Windows language).
use windows::core::PCWSTR;
use windows::Win32::System::Performance::*;

pub struct Query(PDH_HQUERY);
// SAFETY: PDH handles are plain ids; the sampler uses them from one thread at a time.
unsafe impl Send for Query {}
unsafe impl Send for Counter {}
#[derive(Clone, Copy)]
pub struct Counter(PDH_HCOUNTER);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

impl Query {
    pub fn new() -> Option<Query> {
        let mut q = PDH_HQUERY::default();
        (unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut q) } == 0).then_some(Query(q))
    }

    pub fn add(&self, path: &str) -> Option<Counter> {
        let p = wide(path);
        let mut c = PDH_HCOUNTER::default();
        (unsafe { PdhAddEnglishCounterW(self.0, PCWSTR(p.as_ptr()), 0, &mut c) } == 0).then_some(Counter(c))
    }

    pub fn collect(&self) -> bool {
        unsafe { PdhCollectQueryData(self.0) == 0 }
    }
}

impl Drop for Query {
    fn drop(&mut self) {
        unsafe {
            PdhCloseQuery(self.0);
        }
    }
}

impl Counter {
    /// All instances of a wildcard counter as (instance name, value). Empty until two samples exist.
    pub fn array(&self) -> Vec<(String, f64)> {
        const MORE_DATA: u32 = 0x800007D2;
        let (mut size, mut count) = (0u32, 0u32);
        let st = unsafe { PdhGetFormattedCounterArrayW(self.0, PDH_FMT_DOUBLE, &mut size, &mut count, None) };
        if st != MORE_DATA || size == 0 {
            return Vec::new();
        }
        let mut buf = vec![0u64; (size as usize).div_ceil(8)]; // u64 for alignment
        let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
        if unsafe { PdhGetFormattedCounterArrayW(self.0, PDH_FMT_DOUBLE, &mut size, &mut count, Some(items)) } != 0 {
            return Vec::new();
        }
        (0..count as usize)
            .filter_map(|i| unsafe {
                let it = &*items.add(i);
                (it.FmtValue.CStatus == 0).then(|| (it.szName.to_string().unwrap_or_default(), it.FmtValue.Anonymous.doubleValue))
            })
            .collect()
    }
}
