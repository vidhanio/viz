//! Resolve pending harness requests and advance structured form steps.

use viz_client::protocol::{
    AnswerRequest, FormValue, RequestAnswer, RequestBody, RequestId, question_fields,
};

use crate::{
    effects::{Effect, ServerEffect},
    model::{Model, requests::pending},
};

#[derive(Debug)]
pub enum RequestMsg {
    Answer(RequestId, RequestAnswer),
    Finished(RequestId, Result<(), String>),
    Collapse(RequestId),
    Back(RequestId),
    Choose(RequestId, FormValue, bool),
    Next(RequestId, String),
}

fn answer(model: &mut Model, id: RequestId, answer: RequestAnswer) -> Vec<Effect> {
    if !model.connected() || model.requests.sending.contains(&id) {
        return Vec::new();
    }
    let Some(thread) = pending(model)
        .find(|request| request.id == id)
        .map(|request| request.thread)
    else {
        return Vec::new();
    };
    model.requests.sending.insert(id);
    vec![Effect::Server(ServerEffect::Answer(AnswerRequest {
        thread,
        request: id,
        answer,
    }))]
}

pub fn update(model: &mut Model, msg: RequestMsg) -> Vec<Effect> {
    let (id, text, choice, advance) = match msg {
        RequestMsg::Answer(id, value) => return answer(model, id, value),
        RequestMsg::Finished(id, result) => {
            model.requests.sending.remove(&id);
            match result {
                Ok(()) => {
                    model.requests.forms.remove(&id);
                    model.requests.resolved.insert(id);
                }
                Err(detail) => {
                    return vec![Effect::Toast {
                        title: "Couldn’t answer request".into(),
                        detail: Some(detail),
                    }];
                }
            }
            return Vec::new();
        }
        RequestMsg::Collapse(id) => {
            let form = model.requests.forms.entry(id).or_default();
            form.collapsed = !form.collapsed;
            return Vec::new();
        }
        RequestMsg::Back(id) => {
            let form = model.requests.forms.entry(id).or_default();
            form.step = form.step.saturating_sub(1);
            form.error = None;
            return Vec::new();
        }
        RequestMsg::Choose(id, value, advance) => (id, None, Some(value), advance),
        RequestMsg::Next(id, text) => (id, Some(text), None, true),
    };
    if !model.connected() || model.requests.sending.contains(&id) {
        return Vec::new();
    }
    let Some(schema) = pending(model).find_map(|request| match &request.body {
        RequestBody::Question { schema, .. } if request.id == id => Some(schema.clone()),
        _ => None,
    }) else {
        return Vec::new();
    };
    let steps = question_fields(&schema);
    let form = model.requests.forms.entry(id).or_default();
    form.error = None;
    if let Some(field) = steps.get(form.step) {
        let name = field.name;
        if let Some(choice) = choice {
            form.fields.insert(name.to_owned(), choice);
        }
        let selectable = !field.options.is_empty();
        if let Some(text) = text.filter(|_| !selectable) {
            if text.trim().is_empty() {
                form.fields.remove(name);
            } else {
                match field.parse_text(text) {
                    Ok(value) => {
                        form.fields.insert(name.to_owned(), value);
                    }
                    Err(error) => {
                        form.error = Some(error.to_owned());
                        return Vec::new();
                    }
                }
            }
        }
        if advance
            && field.required
            && form.fields.get(name).is_none_or(
                |value| matches!(value, FormValue::Choices(values) if values.is_empty()),
            )
        {
            form.error = Some("This field is required.".into());
            return Vec::new();
        }
    }
    if !advance {
        return Vec::new();
    }
    if form.step + 1 < steps.len() {
        form.step += 1;
        return Vec::new();
    }
    let fields = form.fields.clone();
    answer(model, id, RequestAnswer::Accept { fields })
}

pub fn reconcile(model: &mut Model) {
    let environment = model.active_env_key();
    if model.requests.environment != environment {
        model.requests = crate::model::requests::Requests {
            environment: environment.to_owned(),
            ..Default::default()
        };
    }
    if !model.connected() {
        model.requests.sending.clear();
    }
}
