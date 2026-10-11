from revalidate_compact import restore
restore('PART-524','/root/analyze_outbound',{
'https.rs':('f67e438115e9d604c23d9c02b824bf3f5925c5b1433d08ac498f705ca118e542','Full1-152','Three bounded HTTPS connectivity/path cases preserved; actual catchall supplies secure-health marker; selfsigned probe deliberately no trust-verification proof.'),
'mod.rs':('d9c3d743e4c84f1bb9d9dc03b115caa1cedfb6d4642c312cf47e557882270ab4','Full1-4','Export preserved.')},[
('server/test_server.rs','6c38b285bdfe354c40d5d2e96d4a07be0b7dc37ce3290197ca75054de23bfa98','570-610/713-725 router/binding;3526-3552 current path/header producer replaces shifted old scope.'),
('client/framework.rs','dd7b89a260befd5283e63c7a405e878918949ad17a618af0b23138312fed0383','68-135 no_proxy/selfsigned/DNS/client5s;535-582 retained30s case timeout.'),
('client/test_client.rs','3f5afa96d10ac15f54b32df48c90af4e862cbf64a4942370f63540298eb80e0b','3270-3300 exact+aggregate HTTPS registry/3850-3945 port/context.'),
('suites/edgion_tls/mod.rs','153105658634e2e984dfb532b5edcffef4000e752bb096ce39a32a6ba29bb055','Full retained parent export.'),
('https/Gateway.yaml','24b93300f7c54cef0842d0eda494791a58df141a8ca1f099167702239060b76d','Full local31190 listener.'),
('https/HTTPRoute.yaml','325935d47d4c4bbb317d3e1cb2165a94c7b843c2bc2930b4dda435efa8252917','Full local secure/defaultprefix30001 route unchanged.'),
('https/EdgionTls.yaml','1dd21fd78fc9c42e12a07c5e5a996c81f94c1b16424e41d1f49b876820e2eabc','Full K8s hostname/section/Secret binding.'),
('05-testing/01-integration-testing.md','e260042b508fa0956873d1b3c059531f1733a7c15423c548081afb71703a10e1','Full retained original current integration guidance.')],
'Original gates full personal/fresh hashes separately bound. Bothfull topology Gateway/HTTPRoute/EdgionTls/localService/EndpointSlice inspected. Current source change preserves asserted markers;272 other children not this context. No execution/live TLS/newcandidate/support primary credit.')
