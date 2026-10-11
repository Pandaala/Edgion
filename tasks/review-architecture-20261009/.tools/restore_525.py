import record as r
from revalidate_compact import restore
restore('PART-525','/root/analyze_outbound',{
'last_known_good.rs':('8a44e763c6fff65ac670ea794b7e1a3f479abb9f16613173f714da3b4bd9592b','Full1-422 two chunks','Four serial/status cases/recovery preserved;288 child ownership and pending266 flatten/first-condition evidence distinct prior roots, no new candidate/self reapproval.'),
'mod.rs':('eb7836441819e12a521719554eadc64487970f754d0c2edba4bf8b5659172b29','Full1-6','Export/caller preserved.')},[
('last-known-good-follows-resource-key.md','61be09a031c5c62c1410a515e0118f4a1f439ffaeeeced14a76c7a719b7c6f18','Full changed original server failure LKG versus present-CA handshake rejection checked.'),
('canonical-parent-status-ownership.md','4c6bba39e7cb8bbdb366d0645896ed941faf67c023e452b7fd2ffd9f2d148a76','Full original foreign retention/local projection distinction.'),
('handlers/edgion_tls.rs','668c32a9861264b09a2180c3bd022e1bbdc0c7b3b262edad4ee4fffc28eb35de','Current1-365 retained fresh parse/typed fields/CertContentInvalid;365-435 freshly read complete owned parent writer/foreign retention replacing old465-525.'),
('mod_test/parent_status_identity_test.rs','e49b3341e57442df0c596f02ad7d06b0dba2521ec425de7632ec6e1a3a913b12','1-156 full actual multiwriter/default/port/duplicate/foreign-first-last EdgionTls body.'),
('scenarios/edgion_tls_last_known_good.rs','328569727113cff6b3dd782d8a623875917bdfac1f11181e28288deb4b86c98b','Full actual initial/garbage/recover/rotate/stash/restore caller.'),
('LastKnownGood/initial/EdgionTls.yaml','e8883ad9e15613cc46105d180c4dc2389d6b3cc490291a562c81de5848b5b6d7','Full local server Secret ref/no clientAuth.'),
('tls/store/tls_store.rs','5e6c04cd8a9d3298e7402e55d22a88e09c6f1b392c06129529df569ab7e79366','237-286 current validated_entry retention before compilation.'),
('tls/store/tls_store_test.rs','e6daa5d37116554b3079ac19d1efe1593407796300a9913e93c9df8fd0d97773','166-190 full actual invalid_update_retains_last_known_good;900-935 badCA assertions scoped context only.')],
'Original rules/topics fresh personal unchanged, changed LKG page/source actually checked. Bothfull topology initial Gateway/HTTPRoute/EdgionTls/current runnerCLIframework retained. No clientAuth here; server versusCA page distinction does not alter this scenario. Availability/status cannot prove Gateway branch because Controller may reject first. No execution/writes/new candidate/support coverage/independent extension approval.',('ARCH-288','ARCH-266 pending supplemental'))
s=r.state();q=s['pending_supplements'].setdefault('ARCH-266',[])
if 'REVALIDATION-PART-525-20261011-H.json' not in q:q.append('REVALIDATION-PART-525-20261011-H.json')
r.save(s)
