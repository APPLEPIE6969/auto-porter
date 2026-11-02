//! Event-listener extraction (spec §2.4).
//!
//! NeoForge/Forge: `@SubscribeEvent` methods (and `@Mod.EventBusSubscriber`
//! classes). Fabric: callback registrations such as
//! `PlayerBlockBreakEvents.Before.EVENT.register(...)` — Fabric has no
//! `@SubscribeEvent`, so detection is interface-based plus the `EVENT` /
//! `register` constant-pool signature.

use crate::classfile::RawClass;
use shared::types::{EventListenerAst, Loader};

const SUBSCRIBE_EVENT: [&str; 2] = [
    "net/neoforged/bus/api/SubscribeEvent",
    "net/minecraftforge/eventbus/api/SubscribeEvent",
];

const EVENT_BUS_SUBSCRIBER: [&str; 3] = [
    "net/neoforged/fml/common/Mod$EventBusSubscriber",
    "net/minecraftforge/fml/common/Mod$EventBusSubscriber",
    "net/minecraftforge/fml/common/EventBusSubscriber",
];

fn first_param_internal(descriptor: &str) -> Option<String> {
    // descriptor like (Lnet/minecraftforge/event/.../LivingHurtEvent;)V
    let inner = descriptor.strip_prefix('(')?;
    if !inner.starts_with('L') {
        return None;
    }
    let end = inner.find(')')?;
    let param = &inner[..end];
    if !param.starts_with('L') || !param.ends_with(';') {
        return None;
    }
    Some(param[1..param.len() - 1].to_string())
}

fn is_fabric_event_interface(name: &str) -> bool {
    name.starts_with("net/fabricmc/fabric/api/event/")
        || (name.contains("Callback") && name.contains('/'))
}

pub fn extract_events(classes: &[RawClass], loader: Loader) -> Vec<EventListenerAst> {
    let mut out = Vec::new();

    for class in classes {
        let has_bus_subscriber = class
            .class_annotations
            .iter()
            .any(|a| EVENT_BUS_SUBSCRIBER.contains(&a.type_name.as_str()));

        match loader {
            Loader::NeoForge | Loader::Forge => {
                for method in &class.methods {
                    let subscribed = method
                        .annotations
                        .iter()
                        .any(|a| SUBSCRIBE_EVENT.contains(&a.type_name.as_str()));
                    if !subscribed {
                        continue;
                    }
                    if let Some(event_class) = first_param_internal(&method.descriptor) {
                        out.push(EventListenerAst {
                            loader,
                            event_class,
                            handler_class: class.this_class.clone(),
                            handler_method: Some(method.name.clone()),
                            registration: if has_bus_subscriber {
                                "annotation+bus_subscriber".to_string()
                            } else {
                                "annotation".to_string()
                            },
                        });
                    }
                }
            }
            Loader::Fabric | Loader::Unknown => {
                // (a) the handler class directly implements a Fabric event interface
                for iface in &class.interfaces {
                    if !is_fabric_event_interface(iface) {
                        continue;
                    }
                    let handler = class
                        .methods
                        .iter()
                        .find(|m| {
                            m.has_code
                                && m.name != "<init>"
                                && m.name != "<clinit>"
                                && !m.name.starts_with("lambda$")
                        })
                        .map(|m| m.name.clone());
                    out.push(EventListenerAst {
                        loader,
                        event_class: iface.clone(),
                        handler_class: class.this_class.clone(),
                        handler_method: handler,
                        registration: "callback".to_string(),
                    });
                }
                // (b) lambda registrations: EVENT field ref + register() call in
                // the constant pool; the event interface is the field owner.
                let has_event_field = class.constants.has_field_ref("EVENT", "fabric");
                let has_register = class
                    .constants
                    .method_refs
                    .iter()
                    .any(|(_, n, _)| n == "register");
                if has_event_field && has_register {
                    if let Some((owner, _, _)) = class
                        .constants
                        .field_refs
                        .iter()
                        .find(|(owner, n, _)| n == "EVENT" && owner.contains("fabric"))
                    {
                        let already = out.iter().any(|e| {
                            &e.event_class == owner && e.handler_class == class.this_class
                        });
                        if !already {
                            out.push(EventListenerAst {
                                loader,
                                event_class: owner.clone(),
                                handler_class: class.this_class.clone(),
                                handler_method: None,
                                registration: "callback".to_string(),
                            });
                        }
                    }
                }
            }
        }
    }

    // Keep one listener per event class: a Fabric mod often both registers the
    // callback (entrypoint class) and implements the interface (handler
    // class) — that is one logical feature, so prefer the entry carrying the
    // handler method.
    let mut per_event: std::collections::BTreeMap<String, EventListenerAst> = Default::default();
    for ev in out {
        match per_event.get(&ev.event_class) {
            Some(existing) if existing.handler_method.is_some() => {}
            _ => {
                per_event.insert(ev.event_class.clone(), ev);
            }
        }
    }
    let mut merged: Vec<EventListenerAst> = per_event.into_values().collect();
    merged.sort_by(|a, b| {
        (&a.event_class, &a.handler_class, &a.handler_method).cmp(&(
            &b.event_class,
            &b.handler_class,
            &b.handler_method,
        ))
    });
    merged
}
