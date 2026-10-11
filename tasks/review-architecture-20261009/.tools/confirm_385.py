import record as r
import subprocess
rows=[
('edgion-tests/src/client/suites/edgion_plugins/plugin_validation_status/plugin_validation_status.rs','dba1b4173afbed6e924ae59bbdec7d1b1993da61364cda21b5f2f4d272654776','Full tencases/threeauthoredYAML/broadapplyErrPASS/K8sflag/otherstatusassertions.'),
('edgion-tests/src/client/framework.rs','dd7b89a260befd5283e63c7a405e878918949ad17a618af0b23138312fed0383','350-505 YAMLmetadata/PUT/retry/POSTfallback/statusStringerasure.'),
('edgion-tests/src/client/admin_apply.rs','0bec1f26fc271bec2ad18caaf80f78c5d15e62534efa6a42eb840a6a8cf92015','Full including real CAS/boundedretry/nonconflict/errorpropagation unitbodies.'),
('edgion-tests/src/client/suites/status/helpers.rs','0236de331bc053f9ba00610543ef4e7a8932a9d49e129110f068091d98f823dc','290-395 authenticatedGET/POST initial404/observed400/subsequent404 JSONhelper.'),
('edgion-resources/tests/condition_set_schema.rs','1e1b527a6221836a98e9c7b8b408d9bb5fce4434b0fb39bce0607d6a3b746f20','Full realvalid/malformed/JSONYAML/semanticempty/consumerprofileunitbodies.'),
('edgion-tests/k8s/scripts/common/verify_plugin_validation_admission.sh','d08a5c3175522fd3ab135fc8d5501e70f3e476feafe7ab7e3ef71c0e552d2fee','Full specificthreereject schema preflight/infrareject/unexpectedacceptance.'),
('edgion-controller/src/api/namespaced_handlers.rs','81943b798b18bb4fbf1c234835e89fdd02fbfb50f27920fe186ae6b783aaa5d5','103-180,228-255,424-462,580-612 createupdate parsebeforepersistence.'),
('edgion-controller/src/api/common.rs','2d96d7e138a37e864cf499469fefb2fe14dd1f18fc6661e9bd7850f8a3522dc9','225-247,348-408 JSONYAML typed400 beforeversionassignment.')]
supports={}
for p,sha,scope in rows:assert r.digest(r.REPO/p)==sha,p;supports[p]=scope
r.confirm('ARCH-385','/root/verify_ip_union','79d833f72979fe3a05104cedf2401709e196c37e108686594507722072673ed8','a6a4ddcf6216c56e5659db5e547473e5f6ba379cf585d9e8cb965a090805c8ad',[
'Independent unchanged confirmation broadapplyErr permits transport/retry/auth/500 without structuralrejectionresponse; priorhealthnotlaterwitness.',
'Existing strict helper supports narrow YAMLpayloadseam preservinginitial404/400/nonpersistence, K8s current preflight reasoncontractunchanged.',
'Originalrules/ConditionSet/mandatoryintegration/authbranches/actual malformedfixtures/preflightunitwrappers personallyinspected; cleanup265/status266distinct; no execution.'],supports)
s=r.state();s['resume_dispatch']['independent_verifier']='ARCH-386';r.save(s)
subprocess.run(['python3',str(r.RUN/'.tools/checkpoint.py')],check=True)
