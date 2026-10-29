// Regression test: the panel SELECT must project every column it reads.
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');
const src=fs.readFileSync(path.join(__dirname,'../src/semantic.rs'),'utf8');
const panel=src.match(/pub async fn panel\([\s\S]*?\n}\n\npub async fn recover/);
assert.ok(panel,'panel() not found');
const query=panel[0].match(/sqlx::query\("SELECT ([^"\n]+) FROM block1_semantic_jobs WHERE package_id=\$1 ORDER BY id DESC LIMIT 6"\)/);
assert.ok(query,'SQL for panel jobs not found');
const selected=query[1].split(',').map(x=>x.trim());
const reads=[...panel[0].matchAll(/row\.try_get\("(\w+)"\)/g)].map(m=>m[1]);
for(const col of reads) assert.ok(selected.includes(col), `SQL SELECT is missing column ${col}`);
console.log('PASS: semantic panel SELECT projects fields read by try_get:', reads.join(', '));
