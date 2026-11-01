use edgion_resources::resources::edgion_plugins::EdgionPlugins;

#[test]
fn region_route_dye_documentation_examples_validate() {
    for document in [
        include_str!("../../docs/en/user-guide/http-route/filters/edgion-plugins/region-route.md"),
        include_str!("../../docs/zh-CN/user-guide/http-route/filters/edgion-plugins/region-route.md"),
    ] {
        let examples: Vec<_> = document
            .split("```yaml\n")
            .skip(1)
            .map(|block| block.split("\n```").next().unwrap())
            .filter(|yaml| yaml.contains("name: X-Served-Region"))
            .collect();
        assert_eq!(examples.len(), 1, "expected one complete dye example per language");
        let plugins: EdgionPlugins = yaml_serde::from_str(examples[0]).unwrap();
        let errors = plugins.validate_plugins();
        assert!(errors.is_empty(), "documented dye must be usable: {errors:?}");
        let entry = &plugins.spec.request_plugins.as_ref().unwrap()[0];
        let dye = entry.dye.as_ref().unwrap();
        assert_eq!(dye.request.len(), 1);
        assert_eq!(dye.response.len(), 1);
    }
}
