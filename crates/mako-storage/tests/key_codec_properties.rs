use std::cmp::Ordering;

use mako_storage::TenantKeyspace;
use proptest::prelude::*;

fn identifier() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 1..128)
}

fn component() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 0..128)
}

proptest! {
    #[test]
    fn arbitrary_identifiers_round_trip_and_remain_in_scope(
        project in identifier(),
        environment in identifier(),
        collection in identifier(),
        document in identifier(),
    ) {
        let tenant = TenantKeyspace::new(project, environment).expect("generated identifiers are valid");
        let key = tenant.document_key(&collection, &document).expect("encode document key");

        prop_assert_eq!(
            tenant.decode_document_key(&collection, &key).expect("decode document key"),
            document,
        );
        prop_assert!(tenant.environment_range().expect("environment range").contains(&key));
        prop_assert!(tenant.documents_range(&collection).expect("document range").contains(&key));
    }

    #[test]
    fn distinct_structured_document_tuples_never_collide(
        project_a in identifier(),
        environment_a in identifier(),
        collection_a in identifier(),
        document_a in identifier(),
        project_b in identifier(),
        environment_b in identifier(),
        collection_b in identifier(),
        document_b in identifier(),
    ) {
        prop_assume!(
            (&project_a, &environment_a, &collection_a, &document_a)
                != (&project_b, &environment_b, &collection_b, &document_b)
        );
        let tenant_a = TenantKeyspace::new(project_a, environment_a).expect("tenant A");
        let tenant_b = TenantKeyspace::new(project_b, environment_b).expect("tenant B");
        let key_a = tenant_a.document_key(collection_a, document_a).expect("key A");
        let key_b = tenant_b.document_key(collection_b, document_b).expect("key B");

        prop_assert_ne!(key_a, key_b);
    }

    #[test]
    fn encoded_document_order_matches_original_byte_order(
        project in identifier(),
        environment in identifier(),
        collection in identifier(),
        left in identifier(),
        right in identifier(),
    ) {
        let tenant = TenantKeyspace::new(project, environment).expect("tenant");
        let left_key = tenant.document_key(&collection, &left).expect("left key");
        let right_key = tenant.document_key(&collection, &right).expect("right key");

        prop_assert_eq!(left.cmp(&right), left_key.cmp(&right_key));
    }

    #[test]
    fn change_key_order_matches_position_and_document_tuple(
        project in identifier(),
        environment in identifier(),
        collection in identifier(),
        left_position in any::<u64>(),
        right_position in any::<u64>(),
        left_document in identifier(),
        right_document in identifier(),
    ) {
        let tenant = TenantKeyspace::new(project, environment).expect("tenant");
        let left_key = tenant.change_key(&collection, left_position, &left_document).expect("left key");
        let right_key = tenant.change_key(&collection, right_position, &right_document).expect("right key");
        let expected = match left_position.cmp(&right_position) {
            Ordering::Equal => left_document.cmp(&right_document),
            ordering => ordering,
        };

        prop_assert_eq!(expected, left_key.cmp(&right_key));
    }

    #[test]
    fn compound_index_order_matches_component_then_document_order(
        project in identifier(),
        environment in identifier(),
        collection in identifier(),
        index in identifier(),
        left_first in component(),
        left_second in component(),
        right_first in component(),
        right_second in component(),
        left_document in identifier(),
        right_document in identifier(),
    ) {
        let tenant = TenantKeyspace::new(project, environment).expect("tenant");
        let left_components = [&left_first, &left_second];
        let right_components = [&right_first, &right_second];
        let left_key = tenant.index_entry_key(
            &collection,
            &index,
            &left_components,
            &left_document,
        ).expect("left index key");
        let right_key = tenant.index_entry_key(
            &collection,
            &index,
            &right_components,
            &right_document,
        ).expect("right index key");
        let expected = (&left_first, &left_second, &left_document)
            .cmp(&(&right_first, &right_second, &right_document));

        prop_assert_eq!(expected, left_key.cmp(&right_key));
    }

    #[test]
    fn prefix_like_tenant_identifiers_cannot_escape_or_overlap(
        victim_project in identifier(),
        victim_environment in identifier(),
        attacker_suffix in identifier(),
        collection in identifier(),
        document in identifier(),
    ) {
        let victim = TenantKeyspace::new(
            victim_project.as_slice(),
            victim_environment.as_slice(),
        ).expect("victim tenant");
        let mut attacker_project = victim_project.clone();
        attacker_project.push(0);
        attacker_project.extend_from_slice(&attacker_suffix);
        let mut attacker_environment = victim_environment.clone();
        attacker_environment.push(0xff);
        attacker_environment.extend_from_slice(&attacker_suffix);
        let attacker = TenantKeyspace::new(attacker_project, attacker_environment).expect("attacker tenant");
        let attacker_key = attacker.document_key(collection, document).expect("attacker key");

        prop_assert!(!victim.environment_range().expect("victim range").contains(&attacker_key));
        prop_assert!(!TenantKeyspace::project_range(&victim_project).expect("project range").contains(&attacker_key));
        prop_assert!(!TenantKeyspace::system_range().expect("system range").contains(&attacker_key));
    }

    #[test]
    fn prefix_like_collection_identifiers_have_disjoint_ranges(
        project in identifier(),
        environment in identifier(),
        victim_collection in identifier(),
        suffix in identifier(),
        document in identifier(),
    ) {
        let tenant = TenantKeyspace::new(project, environment).expect("tenant");
        let mut attacker_collection = victim_collection.clone();
        attacker_collection.push(0);
        attacker_collection.extend_from_slice(&suffix);
        let attacker_key = tenant.document_key(attacker_collection, document).expect("attacker key");

        prop_assert!(!tenant.documents_range(victim_collection).expect("victim collection range").contains(&attacker_key));
    }
}
