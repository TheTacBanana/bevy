//! This example show how you can create components dynamically, spawn entities with those components
//! as well as query for entities with those components.

use std::{alloc::Layout, io::Write, ptr::NonNull};

use bevy::prelude::*;
use bevy::{
    ecs::{
        component::{ComponentDescriptor, ComponentId, ComponentInfo, StorageType},
        query::{QueryBuilder, QueryData},
        world::FilteredEntityMut,
    },
    ptr::OwningPtr,
    utils::HashMap,
};

const PROMPT: &str = "
Commands:
    comp, c   Create new components
    spawn, s  Spawn entities
    query, q  Query for entities
Enter a command with no parameters for usage.";

const COMPONENT_PROMPT: &str = "
comp, c   Create new components
    Enter a comma seperated list of type names optionally followed by a size in u64s.
    e.g. CompA 3, CompB, CompC 2";

const ENTITY_PROMPT: &str = "
spawn, s  Spawn entities
    Enter a comma seperated list of components optionally followed by values.
    e.g. CompA 0 1 0, CompB, CompC 1";

const QUERY_PROMPT: &str = "
query, q  Query for entities
    Enter a query to fetch and update entities
    Components with read or write access will be displayed with their values
    Components with write access will have their fields incremented by one

    Accesses: 'A' with, '&A' read, '&mut A' write
    Operators: '||' or, ',' and, '?' optional
    
    e.g. &A || &B, &mut C, D, ?E";

fn main() {
    let mut world = World::new();
    let mut lines = std::io::stdin().lines();
    let mut component_names = HashMap::<String, ComponentId>::new();
    let mut component_info = HashMap::<ComponentId, ComponentInfo>::new();

    println!("{}", PROMPT);
    loop {
        print!("\n> ");
        let _ = std::io::stdout().flush();
        let Some(Ok(line)) = lines.next() else {
            return;
        };

        if line.is_empty() {
            return;
        };

        let Some((first, rest)) = line.trim().split_once(|c: char| c.is_whitespace()) else {
            match &line.chars().next() {
                Some('c') => println!("{}", COMPONENT_PROMPT),
                Some('s') => println!("{}", ENTITY_PROMPT),
                Some('q') => println!("{}", QUERY_PROMPT),
                _ => println!("{}", PROMPT),
            }
            continue;
        };

        match &first[0..1] {
            "c" => {
                rest.split(',').for_each(|component| {
                    let mut component = component.split_whitespace();
                    let Some(name) = component.next() else {
                        return;
                    };
                    let size = match component.next().map(|s| s.parse::<usize>()) {
                        Some(Ok(size)) => size,
                        _ => 0,
                    };
                    // SAFETY: [u64] is Send + Sync
                    let id = world.init_component_with_descriptor(unsafe {
                        ComponentDescriptor::new_with_layout(
                            name.to_string(),
                            StorageType::Table,
                            Layout::array::<u64>(size).unwrap(),
                            None,
                        )
                    });
                    let Some(info) = world.components().get_info(id) else {
                        return;
                    };
                    component_names.insert(name.to_string(), id);
                    component_info.insert(id, info.clone());
                    println!("Component {} created with id: {:?}", name, id.index());
                });
            }
            "s" => {
                let mut to_insert_ids = Vec::new();
                let mut to_insert_ptr = Vec::new();
                rest.split(',').for_each(|component| {
                    let mut component = component.split_whitespace();
                    let Some(name) = component.next() else {
                        return;
                    };
                    let Some(&id) = component_names.get(name) else {
                        println!("Component {} does not exist", name);
                        return;
                    };
                    let info = world.components().get_info(id).unwrap();
                    let len = info.layout().size() / std::mem::size_of::<u64>();
                    let mut values: Vec<u64> = component
                        .take(len)
                        .filter_map(|value| value.parse::<u64>().ok())
                        .collect();

                    // SAFETY:
                    // - All components will be interpreted as [u64]
                    // - len and layout are taken directly from the component descriptor
                    let ptr = unsafe {
                        let data = std::alloc::alloc_zeroed(info.layout()).cast::<u64>();
                        data.copy_from(values.as_mut_ptr(), values.len());
                        let non_null = NonNull::new_unchecked(data.cast());
                        OwningPtr::new(non_null)
                    };

                    to_insert_ids.push(id);
                    to_insert_ptr.push(ptr);
                });

                let mut entity = world.spawn_empty();
                // SAFETY:
                // - Component ids have been taken from the same world
                // - The pointer with the correct layout
                unsafe {
                    entity.insert_by_ids(&to_insert_ids, to_insert_ptr.into_iter());
                }
                println!("Entity spawned with id: {:?}", entity.id());
            }
            "q" => {
                let mut builder = QueryBuilder::<FilteredEntityMut>::new(&mut world);
                parse_query(rest, &mut builder, &component_names);
                let mut query = builder.build();

                query.iter_mut(&mut world).for_each(|filtered_entity| {
                    let terms = filtered_entity
                        .components()
                        .map(|id| {
                            let ptr = filtered_entity.get_by_id(id).unwrap();
                            let info = component_info.get(&id).unwrap();
                            let len = info.layout().size() / std::mem::size_of::<u64>();

                            // SAFETY:
                            // - All components are created with layout [u64]
                            // - len is calculated from the component descriptor
                            let data = unsafe {
                                std::slice::from_raw_parts_mut(
                                    ptr.assert_unique().as_ptr().cast::<u64>(),
                                    len,
                                )
                            };
                            if filtered_entity.access().has_write(id) {
                                data.iter_mut().for_each(|data| {
                                    *data += 1;
                                });
                            }

                            format!("{}: {:?}", info.name(), data[0..len].to_vec())
                        })
                        .collect::<Vec<_>>()
                        .join(", ");

                    println!("{:?}: {}", filtered_entity.id(), terms);
                });
            }
            _ => continue,
        }
    }
}

fn parse_term<Q: QueryData>(
    str: &str,
    builder: &mut QueryBuilder<Q>,
    components: &HashMap<String, ComponentId>,
) {
    let mut matched = false;
    let str = str.trim();
    match str.chars().next() {
        Some('?') => {
            builder.optional(|b| parse_term(&str[1..], b, components));
            matched = true;
        }
        Some('&') => {
            let mut parts = str.split_whitespace();
            let first = parts.next().unwrap();
            if first == "&mut" {
                if let Some(str) = parts.next() {
                    if let Some(&id) = components.get(str) {
                        builder.mut_id(id);
                        matched = true;
                    }
                };
            } else if let Some(&id) = components.get(&first[1..]) {
                builder.ref_id(id);
                matched = true;
            }
        }
        Some(_) => {
            if let Some(&id) = components.get(str) {
                builder.with_id(id);
                matched = true;
            }
        }
        None => {}
    };

    if !matched {
        println!("Unable to find component: {}", str);
    }
}

fn parse_query<Q: QueryData>(
    str: &str,
    builder: &mut QueryBuilder<Q>,
    components: &HashMap<String, ComponentId>,
) {
    let str = str.split(',');
    str.for_each(|term| {
        let sub_terms: Vec<_> = term.split("||").collect();
        if sub_terms.len() == 1 {
            parse_term(sub_terms[0], builder, components);
        } else {
            builder.or(|b| {
                sub_terms
                    .iter()
                    .for_each(|term| parse_term(term, b, components));
            });
        }
    });
}
