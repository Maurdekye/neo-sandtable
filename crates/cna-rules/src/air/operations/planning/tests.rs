use super::*;
use crate::{Cna, testkit::{assert_action_indistinguishable, assert_indistinguishable}};
use cna_core::{engine::{Game, Command, evaluate}, visibility::Perspective};

fn content() -> &'static CnaContent { crate::air::operations::tests::content() }
fn bytes(game: &Game<Cna>) -> Vec<u8> { serde_json::to_vec(game).unwrap() }
fn respond(game: &Game<Cna>, owner: Side, value: Value) -> Game<Cna> {
    let command = missions::initial::tests::response(game, owner, value);
    evaluate(&Cna::dev(), content(), game, &command).unwrap().game
}
fn component(game: &Game<Cna>, call: impl FnOnce(&mut State, &mut Cx<'_>)) -> (Game<Cna>, Vec<EngineEvent>) {
    let mut result = game.clone();
    let mut rng = CampaignRng::from_state(&game.rng);
    let mut events = Vec::new();
    call(&mut result.state, &mut Cx { rng: &mut rng, events: &mut events });
    result.rng = rng.state();
    (result, events)
}

// Genuine prior initial Respond/declaration source, plus the existing explicitly
// structural refit prerequisite fixture. A component clock moves to the next
// assignment; real fixed declaration windows accept pass and close EMPTY.
// No old tactical declaration is deleted manually and no load right is minted.
// This is not a whole intervening campaign or a publicly executable CAP flight.
fn ready() -> (Game<Cna>, PlaneId, HexId) {
    let (mut game, id, _) = crate::air::operations::tests::staged();
    let mode = crate::air::preparation::known_fuel(&game.state.air.runtime.preparation.current[&id]).unwrap().0;
    assert!(content().units.aircraft[&game.state.air.runtime.aircraft[&id].aircraft].modes[mode].missions.contains_key("f"));
    game.state.cursor.op_stage = Some(2);
    game.state.cursor.index = crate::seq::OPSTAGE.iter().position(|s| s.anchor == missions::ANCHOR).unwrap();
    game.state.cursor.entered = true;
    let (game, _) = component(&game, |s, cx| missions::enter(content(), s, cx, false).unwrap());
    let game = respond(&game, Side::Axis, Value::Null);
    let mut game = respond(&game, Side::Commonwealth, Value::Null);
    missions::finish(content(), &mut game.state).unwrap();
    assert!(game.state.air.runtime.tactical.declarations.is_empty());
    game.state.cursor.index = crate::seq::OPSTAGE.iter().position(|s| s.anchor == DEPLOYMENT).unwrap();
    let facility = ground_facility(content(), &game.state, &id).unwrap().unwrap();
    let crate::state::Location::Hex { hex } = game.state.air.runtime.facilities[&facility].properties(content(), &facility).unwrap().location else { panic!("actual on-map source fixture") };
    (game, id, hex)
}
fn cap(game: &Game<Cna>, id: &PlaneId, hex: &HexId, posture: Posture) -> Value {
    let mode = crate::air::preparation::known_fuel(&game.state.air.runtime.preparation.current[id]).unwrap().0;
    json!({"plane":id,"mode":mode,"target":hex,"path":[hex],"posture":posture,
        "neighbors":if posture == Posture::Offensive { neighbors(content(), hex).into_iter().collect::<Vec<_>>() } else { Vec::new() }})
}
fn opened() -> (Game<Cna>, PlaneId, HexId) {
    let (game, id, hex) = ready();
    let (game, _) = component(&game, |s,cx| enter_assignment(content(), s, cx).unwrap());
    assert_eq!(game.state.decisions.pending.len(),2);
    assert!(game.state.decisions.pending.iter().all(|p| p.kind == ASSIGN));
    (game,id,hex)
}
fn cap_deployed() -> (Game<Cna>, PlaneId, HexId, Vec<EngineEvent>) {
    let (game,id,hex)=opened();
    let game=respond(&game,Side::Axis,json!({"caps":[cap(&game,&id,&hex,Posture::Offensive)]}));
    let game=respond(&game,Side::Commonwealth,Value::Null);
    let (game,events)=component(&game,|s,cx| finish(content(),s,cx).unwrap());
    (game,id,hex,events)
}

fn shortest(start: &HexId, target: &HexId, limit: usize) -> Option<Vec<HexId>> {
    let mut queue=std::collections::VecDeque::from([vec![start.clone()]]);
    let mut seen=BTreeSet::from([start.clone()]);
    while let Some(path)=queue.pop_front() {
        if path.last()==Some(target) { return Some(path); }
        if path.len()>limit { continue; }
        for hex in neighbors(content(),path.last().unwrap()) {
            if seen.insert(hex.clone()) { let mut next=path.clone();next.push(hex);queue.push_back(next); }
        }
    }
    None
}

// BOTH launch sources come from real authenticated initial responses and the
// actual initial finish. Only refit readiness/weather/intervening clock are
// explicit component fixtures. No anonymous enemy flight is invented here.
fn two_cap_answered() -> (Game<Cna>,PlaneId,HexId) {
    let opened=crate::air::operations::tests::opened();
    let (axis,amode,origin)=crate::air::operations::tests::plane(&opened);
    let arange=content().units.aircraft[&opened.state.air.runtime.aircraft[&axis].aircraft].modes[amode].range_hexes.unwrap();
    let (cw,cmode,cpath,apath)=missions::own_planes(&opened.state,Side::Commonwealth).into_iter().find_map(|id|{
        let plane=&opened.state.air.runtime.aircraft[&id];
        let site=ground_facility(content(),&opened.state,&id).ok().flatten()?;
        let crate::state::Location::Hex{hex}=opened.state.air.runtime.facilities[&site].properties(content(),&site).ok()?.location else{return None};
        let source=&content().units.aircraft[&plane.aircraft];
        source.modes.iter().enumerate().find_map(|(mode,m)|{
            if !m.missions.get("f").is_some_and(|v|matches!(v.as_str(),"day"|"night")) || m.fuel_points.is_none() {return None;}
            let range=m.range_hexes?;if range<0||arange<0{return None;}
            let path=shortest(&origin,&hex,usize::try_from(arange+range).ok()?)?;
            let meet=usize::try_from(arange).ok()?.min(path.len()-1);
            if path.len()-1-meet>usize::try_from(range).ok()? {return None;}
            Some((id.clone(),mode,path[meet..].iter().rev().cloned().collect::<Vec<_>>(),path[..=meet].to_vec()))
        })
    }).expect("genuine initially assigned on-map CW F-capable aircraft in printed range overlap");
    let target=apath.last().unwrap().clone();
    let initial=|id:&PlaneId,mode:usize,hex:&HexId|json!([{"mission":"strafe_grounded_aircraft","light":"day","target":{"hex":hex,"scramble_reserve":false},
        "planes":[{"plane":id,"mode":mode,"initial_choice":{"decision":"load","guns":true,"ordnance":{"kind":"none"}}}]}]);
    let one=respond(&opened,Side::Axis,initial(&axis,amode,&origin));
    let mut game=respond(&one,Side::Commonwealth,initial(&cw,cmode,cpath.first().unwrap()));
    missions::finish(content(),&mut game.state).unwrap();
    crate::air::operations::tests::refit_component_input(&mut game.state);
    game.state.cursor.op_stage=Some(2);
    game.state.cursor.index=crate::seq::OPSTAGE.iter().position(|s|s.anchor==missions::ANCHOR).unwrap();
    let (game,_)=component(&game,|s,cx|missions::enter(content(),s,cx,false).unwrap());
    let one=respond(&game,Side::Axis,Value::Null);let mut game=respond(&one,Side::Commonwealth,Value::Null);
    missions::finish(content(),&mut game.state).unwrap();
    game.state.cursor.index=crate::seq::OPSTAGE.iter().position(|s|s.anchor==DEPLOYMENT).unwrap();
    game.state.turn.weather=Some(crate::state::WeatherState{kind:cna_tables::land::weather::WeatherKind::Normal,storm_sections:Vec::new()});
    let (game,_)=component(&game,|s,cx|enter_assignment(content(),s,cx).unwrap());
    let mut a=cap(&game,&axis,&target,Posture::Offensive);a["path"]=json!(apath);
    let mut c=cap(&game,&cw,&target,Posture::Defensive);c["path"]=json!(cpath);
    let one=respond(&game,Side::Axis,json!({"caps":[a]}));let two=respond(&one,Side::Commonwealth,json!({"caps":[c]}));
    (two,axis,target)
}
fn two_cap_deployed() -> (Game<Cna>,PlaneId,HexId) {
    let (two,axis,target)=two_cap_answered();
    let (deployed,_)=close(&two);
    assert_eq!(deployed.state.air.runtime.operations.planning.caps.len(),2);
    assert!(Side::ALL.iter().all(|owner|deployed.state.air.runtime.operations.flights.iter().any(|f|f.owner==*owner&&!f.members.is_empty())));
    (deployed,axis,target)
}
fn close(game: &Game<Cna>) -> (Game<Cna>, Vec<EngineEvent>) {
    component(game, |s,cx| finish(content(),s,cx).unwrap())
}

/// Cases: scen:59.32, airlog:39.16, airlog:40.21, airlog:40.24, airlog:40.27
#[test]
fn actual_own_cap_assignment_then_fixed_query_and_commit_none_close_without_invented_load_or_combat() {
    let (game,id,hex)=opened();
    let preparation=game.state.air.runtime.preparation.clone();
    let maintenance=game.state.air.runtime.maintenance.clone();
    let logistics=serde_json::to_value(&game.state.logistics).unwrap();
    let one=respond(&game,Side::Axis,json!({"caps":[cap(&game,&id,&hex,Posture::Offensive)]}));
    assert_eq!(one.state.air.runtime.aircraft,game.state.air.runtime.aircraft);
    assert_eq!(one.state.air.runtime.preparation,preparation);
    assert_eq!(one.rng,game.rng);
    assert!(one.state.air.runtime.operations.contacts.is_empty());
    let two=respond(&one,Side::Commonwealth,Value::Null);
    let (deployed,events)=close(&two);
    assert_eq!(deployed.state.decisions.pending.len(),2);
    assert!(deployed.state.decisions.pending.iter().all(|p|p.kind==QUERY));
    assert!(events.iter().all(|e| !matches!(&e.event,GameEvent::Note{text} if text.starts_with("Authorized CAP paths"))));
    assert_eq!(deployed.state.air.runtime.operations.planning.caps.len(),1);
    assert_eq!(deployed.state.air.runtime.operations.contacts.len(),1);
    assert_eq!(deployed.state.air.runtime.preparation.initial_choices,preparation.initial_choices);
    assert_eq!(deployed.state.air.runtime.maintenance,maintenance);
    assert_eq!(serde_json::to_value(&deployed.state.logistics).unwrap(),logistics);
    assert!(!deployed.state.air.runtime.aircraft[&id].fuelled);
    assert!(!deployed.state.air.runtime.aircraft[&id].refitted);
    assert_eq!(deployed.rng,game.rng);
    let one=respond(&deployed,Side::Axis,Value::Null);
    let two=respond(&one,Side::Commonwealth,Value::Null);
    let (queried,_) = close(&two);
    assert_eq!(queried.state.decisions.pending.len(),2);
    assert!(queried.state.decisions.pending.iter().all(|p|p.kind==COMMIT && p.space.check(&Value::Null).is_ok()));
    let one=respond(&queried,Side::Axis,Value::Null);
    let two=respond(&one,Side::Commonwealth,Value::Null);
    let (closed,_) = close(&two);
    assert_eq!(closed.state.air.runtime.operations.planning.batch,Some(Batch::Closed));
    assert!(closed.state.air.runtime.operations.planning.interceptions.is_empty());
    assert!(closed.state.air.runtime.operations.planning.used.is_empty());
    assert_eq!(closed.state.air.runtime.operations.contacts,deployed.state.air.runtime.operations.contacts);
    let (again,events)=close(&closed);
    assert_eq!(bytes(&again),bytes(&closed));
    assert!(events.is_empty());
}

/// Cases: airlog:40.21, airlog:40.23, airlog:40.24, airlog:40.27
#[test]
fn assignment_rejects_duplicate_plane_bad_neighbors_foreign_plane_and_unknown_load_without_mutation() {
    let (game,id,hex)=opened();
    let before=bytes(&game);
    let valid=cap(&game,&id,&hex,Posture::Offensive);
    let mut short=valid.clone(); short["neighbors"]=json!([]);
    let mut duplicate=valid.clone(); duplicate["neighbors"]=json!([hex,hex]);
    let mut wrong=valid.clone(); wrong["plane"]=json!("cw-forged-plane");
    let mut mode=valid.clone(); mode["mode"]=json!(usize::MAX);
    for action in [json!({"caps":[valid.clone(),valid.clone()]}),json!({"caps":[short]}),json!({"caps":[duplicate]}),json!({"caps":[wrong]}),json!({"caps":[mode]})] {
        let cmd=missions::initial::tests::response(&game,Side::Axis,action);
        assert!(evaluate(&Cna::dev(),content(),&game,&cmd).is_err());
        assert_eq!(bytes(&game),before);
    }
    // Independent legacy-knowledge component fixture with valid coarse mirrors.
    let mut unknown=game.clone();
    unknown.state.air.runtime.preparation.current.insert(id.clone(),crate::air::preparation::CurrentPreparation::UnknownLegacy);
    inventory::check_at(content(),&unknown.state.air,Some(context(&unknown.state).unwrap())).unwrap();
    let cmd=missions::initial::tests::response(&unknown,Side::Axis,json!({"caps":[valid]}));
    assert!(evaluate(&Cna::dev(),content(),&unknown,&cmd).is_err());
    assert_eq!(bytes(&game),before);
}

/// Cases: airlog:39.16, airlog:40.27
#[test]
fn route_answers_are_real_committed_mission_paths_and_empty_own_lists_have_legal_none_domains() {
    let (game,id,routes)=crate::air::operations::tests::staged();
    let (game,_)=component(&game,|s,cx|enter_assignment(content(),s,cx).unwrap());
    let axis=game.state.decisions.pending.iter().find(|p|p.seat.side==Side::Axis).unwrap();
    assert!(axis.space.check(&Value::Null).is_err());
    let no_route=missions::initial::tests::response(&game,Side::Axis,json!({}));
    assert!(evaluate(&Cna::dev(),content(),&game,&no_route).is_err());
    let action=json!({"routes":routes[&Side::Axis]});
    let one=respond(&game,Side::Axis,action);
    let two=respond(&one,Side::Commonwealth,json!({"routes":null,"caps":null}));
    let (deployed,_)=close(&two);
    assert!(matches!(position(content(),&deployed.state,&id).unwrap(),Position::Airborne{..}));
    assert_eq!(deployed.state.air.runtime.operations.routes,routes);
    for p in &deployed.state.decisions.pending { p.space.check(&Value::Null).unwrap(); }
}

// A deliberately private encounter-input fixture. It does NOT claim enemy
// declaration/flight provenance or a campaign trajectory: the actual source
// queue and exact admission are exercised after fixing the component snapshot.
fn enemy_path_component(game: &Game<Cna>, origin: &HexId) -> Game<Cna> {
    let mut result=game.clone();
    let expected=context(&result.state).unwrap();
    let mut after=result.state.air.runtime.clone();
    let target=content().map.neighbors(origin).first().unwrap().id.clone();
    let contact=Contact{owner:Side::Commonwealth,class:Class::Bomber,target:target.clone()};
    after.operations.contacts.insert(contact);
    // Empty survivors are allowed; the immutable deployment path/classes remain.
    after.operations.flights.push(Flight{owner:Side::Commonwealth,target:target.clone(),path:vec![origin.clone(),target],members:Vec::new(),classes:BTreeSet::from([Class::Bomber])});
    apply(content(),&mut result.state,after,expected).unwrap();
    result
}

/// Cases: airlog:34.73, airlog:34.74, airlog:40.27
/// Interpretations: interp:air-0021
#[test]
fn requester_only_finish_disclosure_deduplicates_paths_and_omits_members_models_pilots_and_count() {
    let (game,_,_)=two_cap_deployed();
    let contacts:Vec<_>=game.state.air.runtime.operations.contacts.iter().filter(|c|c.owner==Side::Commonwealth).collect();
    assert_eq!(contacts.len(),1);
    let a=respond(&game,Side::Axis,json!(["0"]));
    assert!(a.state.air.runtime.operations.planning.disclosures.is_empty());
    assert_eq!(a.rng,game.rng);
    let b=respond(&a,Side::Commonwealth,Value::Null);
    let (queried,events)=close(&b);
    let d=&queried.state.air.runtime.operations.planning.disclosures[&Side::Axis];
    assert_eq!(d.len(),1);assert_eq!(d[0].paths.len(),1);
    let notes:Vec<_>=events.iter().filter(|e|matches!(&e.event,GameEvent::Note{text} if text.starts_with("Authorized CAP paths"))).collect();
    assert_eq!(notes.len(),2);
    let private=notes.iter().find(|e|e.visible_to(Perspective::Side(Side::Axis))).unwrap();
    assert!(!private.visible_to(Perspective::Side(Side::Commonwealth)));
    let GameEvent::Note{text}=&private.event else {unreachable!()};
    for forbidden in ["plane","members","pilot","model","count","squadron","mission","fuel","ammo"] { assert!(!text.contains(forbidden)); }
    let mut zero=game.clone();
    let mut snapshot=zero.state.air.runtime.clone();
    let lost:Vec<_>=snapshot.operations.flights.iter().filter(|f|f.owner==Side::Commonwealth).flat_map(|f|f.members.iter().map(|m|m.plane.clone())).collect();
    assert!(!lost.is_empty());
    // Explicit post-casualty component input, not actual combat/recovery. Keep
    // immutable path/classes, public contacts and permanent initial tombstones.
    for id in &lost { snapshot.aircraft.remove(id);snapshot.preparation.current.remove(id); }
    for f in snapshot.operations.flights.iter_mut().filter(|f|f.owner==Side::Commonwealth) { f.members.clear(); }
    let expected=context(&zero.state).unwrap();
    apply(content(),&mut zero.state,snapshot,expected).unwrap();
    assert_eq!(zero.state.air.runtime.preparation.initial_choices,game.state.air.runtime.preparation.initial_choices);
    let cmd=missions::initial::tests::response(&game,Side::Axis,json!(["0"]));
    assert_action_indistinguishable(&Cna::dev(),content(),&game,&zero,&cmd,Side::Axis);
    let one=respond(&zero,Side::Axis,json!(["0"]));let two=respond(&one,Side::Commonwealth,Value::Null);
    let (zero,zero_events)=close(&two);
    assert_eq!(zero.state.air.runtime.operations.planning.disclosures[&Side::Axis],*d);
    assert_eq!(events.iter().filter(|e|e.visible_to(Perspective::Side(Side::Axis))).collect::<Vec<_>>(),zero_events.iter().filter(|e|e.visible_to(Perspective::Side(Side::Axis))).collect::<Vec<_>>());
    let mut both_classes=game.clone();
    let mut snapshot=both_classes.state.air.runtime.clone();
    let f=snapshot.operations.flights.iter_mut().find(|f|f.owner==Side::Commonwealth).unwrap();
    f.classes.insert(Class::Bomber);
    let target=f.target.clone();
    snapshot.operations.contacts.insert(Contact{owner:Side::Commonwealth,class:Class::Bomber,target});
    let expected=context(&both_classes.state).unwrap();
    // Component class-snapshot input only; no additional model/count is claimed.
    apply(content(),&mut both_classes.state,snapshot,expected).unwrap();
    let one=respond(&both_classes,Side::Axis,json!(["0","1"]));
    let two=respond(&one,Side::Commonwealth,Value::Null);
    let (distinct,_)=close(&two);
    assert_eq!(known_routes(&distinct.state,Side::Axis).len(),1);
    let mut split=game.clone();
    // Alternate private source record multiplicity is collapsed to the same
    // cross-class target+path identity by the producer, never revealed by query.
    let flight=split.state.air.runtime.operations.flights.iter().find(|f|f.owner==Side::Commonwealth).unwrap().clone();
    split.state.air.runtime.operations.flights.push(flight);
    let before=bytes(&split);
    assert!(component_result(&mut split).is_err()); // malformed duplicate snapshot rejects atomically
    assert_eq!(bytes(&split),before);
    let (recovered, replay)=close(&serde_json::from_slice(&bytes(&b)).unwrap());
    assert_eq!(bytes(&recovered),bytes(&queried));assert_eq!(replay,events);
}

fn component_result(game:&mut Game<Cna>)->Result<(),EngineError>{
    let mut rng=CampaignRng::from_state(&game.rng);let mut events=Vec::new();
    finish(content(),&mut game.state,&mut Cx{rng:&mut rng,events:&mut events})
}

/// Cases: airlog:40.23, airlog:40.27
/// Interpretations: interp:air-0022
#[test]
fn actual_commitment_uses_authorized_path_own_hex_and_exact_once_without_combat_or_rng() {
    let (game,id,hex)=two_cap_deployed();
    let a=respond(&game,Side::Axis,json!(["0"]));
    let b=respond(&a,Side::Commonwealth,Value::Null);
    let (queried,_)=close(&b);
    let order=json!({"route":"0","hex":hex,"fighters":[id]});
    let command=missions::initial::tests::response(&queried,Side::Axis,json!([order.clone(),order.clone()]));
    assert!(evaluate(&Cna::dev(),content(),&queried,&command).is_err());
    let a=respond(&queried,Side::Axis,json!([order]));
    assert!(a.state.air.runtime.operations.planning.used.is_empty());
    let b=respond(&a,Side::Commonwealth,Value::Null);
    let (closed,_)=close(&b);
    assert_eq!(closed.state.air.runtime.operations.planning.used,BTreeSet::from([id.clone()]));
    let encounters=&closed.state.air.runtime.operations.planning.interceptions;
    assert_eq!(encounters.len(),1);assert_eq!(encounters[0].hex,hex);assert_eq!(encounters[0].fighters,vec![id]);
    assert_eq!(closed.state.air.runtime.aircraft,queried.state.air.runtime.aircraft);
    assert_eq!(closed.state.air.runtime.preparation,queried.state.air.runtime.preparation);
    assert_eq!(closed.rng,queried.rng);
    assert_eq!(closed.state.air.runtime.operations.contacts,queried.state.air.runtime.operations.contacts);
    let (again,events)=close(&closed);assert_eq!(bytes(&again),bytes(&closed));assert!(events.is_empty());
}

/// Cases: airlog:40.23, airlog:40.27
#[test]
fn adjacent_optional_reach_counts_assigned_cap_not_selected_subset_and_defensive_never_intercepts() {
    let (base,id,hex,_)=cap_deployed();
    let game=enemy_path_component(&base,&hex);
    let a=respond(&game,Side::Axis,json!(["0"]));let b=respond(&a,Side::Commonwealth,Value::Null);let(mut game,_)=close(&b);
    let neighbor=content().map.neighbors(&hex).first().unwrap().id.clone();
    let order=InterceptOrder{route:"0".into(),hex:neighbor.clone(),fighters:vec![id.clone()]};
    assert!(validate_intercepts(content(),&game.state,Side::Axis,std::slice::from_ref(&order)).is_err());
    // Assignment-count component input for this single predicate; not live
    // extra aircraft or a canonical launch/whole-State invariant claim.
    let original=game.state.air.runtime.operations.planning.caps[0].clone();
    for n in 0..2 { let mut extra=original.clone();extra.plane=PlaneId(format!("component-assigned-{n}"));game.state.air.runtime.operations.planning.caps.push(extra); }
    assert_eq!(validate_intercepts(content(),&game.state,Side::Axis,std::slice::from_ref(&order)).unwrap()[0].fighters,vec![id.clone()]);
    game.state.air.runtime.operations.planning.caps[0].posture=Posture::Defensive;
    assert!(validate_intercepts(content(),&game.state,Side::Axis,std::slice::from_ref(&order)).is_err());
}

/// Cases: scen:59.32, airlog:40.27
#[test]
fn stale_period_old_window_duplicate_answer_and_generic_planning_forgery_reject_atomically() {
    let (game,_,_)=opened();
    let pending=game.state.decisions.pending[0].clone();
    let mut changed=game.clone();changed.state.cursor.op_stage=Some(3);
    let before=bytes(&changed);assert!(answer(content(),&mut changed.state,&pending,&Value::Null).is_err());assert_eq!(bytes(&changed),before);
    let mut wrong=pending.clone();wrong.kind=QUERY.into();let mut copy=game.clone();
    assert!(answer(content(),&mut copy.state,&wrong,&Value::Null).is_err());assert_eq!(bytes(&copy),bytes(&game));
    let once=respond(&game,pending.seat.side,Value::Null);let mut twice=once.clone();
    assert!(answer(content(),&mut twice.state,&pending,&Value::Null).is_err());assert_eq!(bytes(&twice),bytes(&once));
    let mut forged=game.clone();
    assert!(inventory::update_from_state(content(),&mut forged.state,|r|{r.operations.planning.batch=Some(Batch::Closed);Ok(())}).is_err());
    assert_eq!(bytes(&forged),bytes(&game));
}

/// Cases: airlog:39.19, airlog:40.27
#[test]
fn late_query_draft_failure_preserves_whole_game_rng_existing_events_and_retry_disclosure() {
    let (game,_,_)=two_cap_deployed();
    let a=respond(&game,Side::Axis,json!(["0"]));let mut b=respond(&a,Side::Commonwealth,Value::Null);
    let before=bytes(&b);let mut rng=CampaignRng::from_state(&b.rng);
    let prefix=EngineEvent::public(GameEvent::Note{text:"existing prefix".into()});let mut events=vec![prefix.clone()];
    let error=finish_transaction(content(),&mut b.state,&mut Cx{rng:&mut rng,events:&mut events},|draft|{
        assert_eq!(draft.air.runtime.operations.planning.disclosures[&Side::Axis][0].paths.len(),1);
        draft.air.runtime.operations.period=None;
    }).unwrap_err();
    assert!(matches!(error,EngineError::Invariant{..}));assert_eq!(bytes(&b),before);assert_eq!(rng.state(),b.rng);assert_eq!(events,vec![prefix]);
    let (retry,events)=close(&b);assert_eq!(retry.state.air.runtime.operations.planning.batch,Some(Batch::Commit));
    assert!(events.iter().any(|e|matches!(&e.event,GameEvent::Note{text} if text.starts_with("Authorized CAP paths"))));
}

/// Cases: land:3.6, airlog:34.73, airlog:40.27
#[test]
fn hidden_cap_posture_preserves_other_side_domains_actions_and_public_prelaunch_guard() {
    let (game,id,hex)=opened();
    let a=respond(&game,Side::Axis,json!({"caps":[cap(&game,&id,&hex,Posture::Offensive)]}));
    let b=respond(&game,Side::Axis,json!({"caps":[cap(&game,&id,&hex,Posture::Defensive)]}));
    let cmd=missions::initial::tests::response(&a,Side::Commonwealth,Value::Null);
    assert_action_indistinguishable(&Cna::dev(),content(),&a,&b,&cmd,Side::Commonwealth);
    assert_indistinguishable(&Cna::dev(),content(),&a.state,&b.state,Side::Commonwealth);
    let (ready,_,_)=ready();
    let before=bytes(&ready);
    let outcome=evaluate(&Cna::dev(),content(),&ready,&Command::Advance).unwrap();
    assert_eq!(outcome.game.state.air.runtime.preparation,ready.state.air.runtime.preparation);
    assert_eq!(outcome.game.state.air.runtime.operations,OperationalState::default());
    assert!(outcome.game.state.decisions.pending.iter().all(|p|!matches!(p.kind.as_str(),ASSIGN|QUERY|COMMIT)));
    assert!(matches!(evaluate(&Cna::full(),content(),&ready,&Command::Advance),Err(EngineError::Unsupported{..})));
    assert_eq!(bytes(&ready),before);
}

/// Cases: land:3.6, airlog:34.73, airlog:40.27
#[test]
fn hidden_route_and_private_query_and_commitment_receipts_have_identical_opponent_streams() {
    let (game,id,hex)=opened();
    let simple=cap(&game,&id,&hex,Posture::Offensive);
    let mut looped=simple.clone();
    let neighbor=content().map.neighbors(&hex).first().unwrap().id.clone();
    looped["path"]=json!([hex,neighbor,hex]);
    let a=respond(&game,Side::Axis,json!({"caps":[simple]}));
    let b=respond(&game,Side::Axis,json!({"caps":[looped]}));
    let cmd=missions::initial::tests::response(&a,Side::Commonwealth,Value::Null);
    assert_action_indistinguishable(&Cna::dev(),content(),&a,&b,&cmd,Side::Commonwealth);
    let (a,ae)=close(&respond(&a,Side::Commonwealth,Value::Null));
    let (b,be)=close(&respond(&b,Side::Commonwealth,Value::Null));
    assert_indistinguishable(&Cna::dev(),content(),&a.state,&b.state,Side::Commonwealth);
    assert_eq!(ae.iter().filter(|e|e.visible_to(Perspective::Side(Side::Commonwealth))).collect::<Vec<_>>(),be.iter().filter(|e|e.visible_to(Perspective::Side(Side::Commonwealth))).collect::<Vec<_>>());
    assert_eq!(a.state.air.runtime.operations.contacts,b.state.air.runtime.operations.contacts);
    assert_ne!(a.state.air.runtime.operations.flights[0].path,b.state.air.runtime.operations.flights[0].path);

    let (game,id,hex)=two_cap_deployed();
    let asked=respond(&game,Side::Axis,json!(["0"]));
    let none=respond(&game,Side::Axis,Value::Null);
    let cmd=missions::initial::tests::response(&asked,Side::Commonwealth,Value::Null);
    assert_action_indistinguishable(&Cna::dev(),content(),&asked,&none,&cmd,Side::Commonwealth);
    let (asked,ae)=close(&respond(&asked,Side::Commonwealth,Value::Null));
    let (none,be)=close(&respond(&none,Side::Commonwealth,Value::Null));
    assert_indistinguishable(&Cna::dev(),content(),&asked.state,&none.state,Side::Commonwealth);
    assert_eq!(ae.iter().filter(|e|e.visible_to(Perspective::Side(Side::Commonwealth))).collect::<Vec<_>>(),be.iter().filter(|e|e.visible_to(Perspective::Side(Side::Commonwealth))).collect::<Vec<_>>());
    assert!(!known_routes(&asked.state,Side::Axis).is_empty());
    assert!(known_routes(&none.state,Side::Axis).is_empty());
    let committed=respond(&asked,Side::Axis,json!([{"route":"0","hex":hex,"fighters":[id]}]));
    let passed=respond(&asked,Side::Axis,Value::Null);
    let cmd=missions::initial::tests::response(&committed,Side::Commonwealth,Value::Null);
    assert_action_indistinguishable(&Cna::dev(),content(),&committed,&passed,&cmd,Side::Commonwealth);
    let (committed,ce)=close(&respond(&committed,Side::Commonwealth,Value::Null));
    let (passed,pe)=close(&respond(&passed,Side::Commonwealth,Value::Null));
    assert_indistinguishable(&Cna::dev(),content(),&committed.state,&passed.state,Side::Commonwealth);
    assert_eq!(ce.iter().filter(|e|e.visible_to(Perspective::Side(Side::Commonwealth))).collect::<Vec<_>>(),pe.iter().filter(|e|e.visible_to(Perspective::Side(Side::Commonwealth))).collect::<Vec<_>>());
    assert_eq!(committed.rng,passed.rng);
    assert_ne!(committed.state.air.runtime.operations.planning.used,passed.state.air.runtime.operations.planning.used);
}

/// Cases: airlog:37.12, airlog:39.19, airlog:40.21, airlog:40.27
#[test]
fn second_owner_missing_source_and_missing_closed_answer_preserve_state_rng_and_prefix() {
    let (mut game,_,_)=two_cap_answered();
    let before=bytes(&game);
    let cap=&game.state.air.runtime.operations.planning.assignments[&Side::Commonwealth].caps[0];
    let aircraft=game.state.air.runtime.aircraft[&cap.plane].aircraft.clone();
    let mode=cap.mode;
    let mut source=CnaContent::load(&cna_content::repo_data_dir(),"graziani").unwrap();
    source.units.aircraft.get_mut(&aircraft).unwrap().modes[mode].range_hexes=None;
    let mut rng=CampaignRng::from_state(&game.rng);
    let prefix=EngineEvent::public(GameEvent::Note{text:"existing source-error prefix".into()});
    let mut events=vec![prefix.clone()];
    let error=finish(&source,&mut game.state,&mut Cx{rng:&mut rng,events:&mut events}).unwrap_err();
    assert!(matches!(error,EngineError::Unsupported{case,..} if case=="airlog:37.12"));
    assert_eq!(bytes(&game),before);assert_eq!(rng.state(),game.rng);assert_eq!(events,vec![prefix]);
    let mut malformed=game.clone();
    malformed.state.air.runtime.operations.planning.assignments.remove(&Side::Commonwealth);
    let missing=bytes(&malformed);
    assert!(component_result(&mut malformed).is_err());assert_eq!(bytes(&malformed),missing);
    let mut crossed=game.clone();crossed.state.cursor.index=crate::seq::OPSTAGE.iter().position(|s|s.anchor=="opstage.land_support_air.air_combat").unwrap();
    assert!(inventory::check_at(content(),&crossed.state.air,Some(Context::from_state(&crossed.state).unwrap())).is_err());
}
