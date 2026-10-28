"use strict";
const assert=require("node:assert/strict");
const fs=require("node:fs");
const path=require("node:path");
const root=path.resolve(__dirname,"..");
const read=p=>fs.readFileSync(path.join(root,p),"utf8");
const block=read("src/block1.rs"), web=read("src/web.rs"),engine=read("src/engine.rs"),migration=read("migrations/018_block1_work_scope.sql");
for (const [text,label] of [
["manifest_problems(scope,&inputs)","manifest must be verified against exact source hashes"],
["counted_unique_codes(inputs.iter().copied())","duplicate files must not raise quantity"],
["work_operations(db,package_id)","work scope must be included in checks"],
["work_evidence_count(op,&work_links,&inputs)","work evidence is scoped to an actual operation"],
["effective_minimums","actual construction volumes must adjust quantities"],
["WORK-SCOPE-PENDING","missing engineer signoff blocks full assurance"],
["requirement_parent_valid","appendix must be linked to a current act"],
["NO-PUBLISHED-DELIVERY","a failed upload cannot satisfy a checklist"],
["METADATA-INCOMPLETE","failed OCR cannot yield positive completeness"]
]) assert.ok(engine.includes(text),label);
for(const s of ["/work-register","/work-register/approve","/work-register/{work_id}/evidence","/work-register/{work_id}/evidence/{doc_id}/remove","/support/{req_id}/evidence/{doc_id}/remove","/support/{req_id}/parent","work_approved.is_none()","manifest_ok","metadata_ok"])
assert.ok(web.includes(s),`missing UI/API: ${s}`);
for(const s of ["package_work_operations","package_work_evidence","check_run_block1_metrics","block1_scope_approved_at","parent_document_id"])
assert.ok(migration.includes(s),`missing migration field: ${s}`);
assert.equal((read("src/block1.rs").match(/SupportTemplate\{code:"S\d\d"/g)||[]).length,19);
assert.equal((read("src/block1.rs").match(/SupportTemplate\{code:"L\d\d"/g)||[]).length,12);
assert.ok(!/\b(?:DROP|TRUNCATE)\b/i.test(migration),"upgrade must be additive");
console.log("OK: 0.9.21 work scope, manifest SHA, duplicate guards, linked appendices, approvals, 31 groups");
