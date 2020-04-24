// auth functions]
#[macro_use]
extern crate diesel;
#[macro_use] extern crate log;
#[macro_use] extern crate lazy_static;
#[macro_use] extern crate maplit;

use std::convert::Infallible;
use zxcvbn::zxcvbn;

use warp::{Reply, Filter, Rejection};
use warp::http;
use warp::hyper::Body;

use hyper;
use askama::Template;
use env_logger;
use db::note::{NoteInput, Note};
use db::note;
use db::user::{RegistrationKey, User, NewUser};
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use diesel::insert_into;
use serde::{Deserialize};
use session::{Session};
use diesel::r2d2::{ConnectionManager, Pool};

type SqlitePool = Pool<ConnectionManager<SqliteConnection>>;

mod db;
mod session;
mod ap;

// We use a global shared sqlite connection because it's simple and performance is not 
// very important

fn pooled_sqlite() -> SqlitePool {
    let manager = ConnectionManager::<SqliteConnection>::new(std::env::var("DATABASE_URL").unwrap());
    Pool::new(manager).expect("Postgres connection pool could not be created")
}


lazy_static! {
    static ref POOL: SqlitePool = pooled_sqlite();
}

// fn POOL.get().unwrap() -> diesel::SqliteConnection {
//     return *POOL.get().unwrap();


// TODO split into separate templates. not sure how
#[derive(Template)]
#[template(path = "timeline.html")] 
struct TimelineTemplate<'a>{
    global: Global<'a>,
    page: &'a str,
    notes: Vec<Note>,
} 

struct Global<'a> {
    title: &'a str,
    user: User,
    logged_in: bool,
}

impl<'a> Global<'a> {
    fn from_session(session: Option<Session>) -> Self {
        match session {
            Some(s) => Global {
            logged_in: true,
            title: "gourami",
            user: s.user.clone(),
        },
            None => Global {
            logged_in: false,
            title: "gourami",
            user: User::default(),
        }
        }
    }
}

pub fn render_template<T: askama::Template>(t: &T) -> http::Response<hyper::body::Body> {
    match t.render() {
        Ok(body) => http::Response::builder()
            .status(http::StatusCode::OK)
            // TODO add headers etc
            .body(body.into()),
        Err(_) => http::Response::builder()
            .status(http::StatusCode::INTERNAL_SERVER_ERROR)
            .body(Body::empty()),
    }
    .unwrap()
}

fn delete_note(session: Option<Session>, note_id: i32) -> impl Reply {
    use db::schema::notes::dsl::*;
    diesel::delete(notes.filter(id.eq(note_id))).execute(&POOL.get().unwrap()).unwrap();
    warp::redirect::redirect(warp::http::Uri::from_static("/"))
}

#[derive(Deserialize)]
struct NewNoteRequest {
    note_input: String, // has to be String
}

fn new_note(session: Option<Session>, req: NewNoteRequest) -> impl Reply {
    use db::schema::notes::dsl::*;
    // create activitypub activity object
    // TODO -- micropub?
    if let Some(s) = session {
        let new_note = NoteInput{
            creator_id: s.user.id,
            creator_username: s.user.username,
            parent_id: None,
            content: note::parse_note_text(&req.note_input), // how to avoid clone here?
        };
        insert_into(notes).values(new_note).execute(&POOL.get().unwrap()).unwrap();
        return warp::redirect::redirect(warp::http::Uri::from_static("/"))
    } else {
        return warp::redirect::redirect(warp::http::Uri::from_static("/"))
    }

    // generate activitypub object from post request
    // send to outbox
    // if request made from web form
}

// ActivityPub outbox 
fn send_to_outbox(activity: bool) { // activitystreams object
    // fetch/store from db.
    // db objects need to serialize/deserialize this object
    // if get -> fetch from db
    // if post -> put to db, send to inbox of followers
    // send to inbox of followers
}


#[derive(Template)]
#[template(path = "register.html")] 
struct RegisterTemplate<'a>{
    page: &'a str,
    keyed: bool,
    key: &'a str,
    global: Global<'a>,
} 

fn register_page(query_params: serde_json::Value) -> impl warp::Reply {
    let global = Global::from_session(None); 
    let keyed;
    if let Some(k) = query_params.get("key") {
        let key_str = k.as_str().unwrap();
        keyed = RegistrationKey::is_valid(&POOL.get().unwrap(), &key_str);
        render_template(&RegisterTemplate{keyed: keyed, key: key_str,  page: "register", global:global})
    }
    else {
        keyed = false;
        render_template(&RegisterTemplate{keyed: keyed, key: "",  page: "register", global:global})
    }
    // TODO -- do... something if session is not none
}


#[derive(Deserialize)]
struct RegisterForm {
    username: String,
    password: String,
    email: String,
}


impl RegisterForm {
    fn validate(&self) -> Result<(), &'static str> {
        if zxcvbn(&self.password, &[]).unwrap().score() < 1 {
            return Err("Please come up with a more secure password.")
        }
        Ok(())
    }
}

// TODO move all authentication
fn do_register(form: RegisterForm, query_params: serde_json::Value) -> impl Reply {
    let conn = &POOL.get().unwrap();
    use db::schema::users::dsl::*;
    if form.validate().is_err(){ // TODO catch better
        return do_login(LoginForm{username: form.username, password: form.password})
    }
    if let Some(k) = query_params.get("key") {
        let k_string = &k.as_str().unwrap();
        let keyed = RegistrationKey::is_valid(conn, k_string);
        RegistrationKey::clear_key(conn, k_string);
        if keyed {
            let hash = bcrypt::hash(&form.password, bcrypt::DEFAULT_COST).unwrap();
            let new_user = NewUser {username: &form.username, password: &hash, email: &form.email};
            // todo data validation
            insert_into(users)
                .values(new_user)
                .execute(conn).unwrap();

        // insert into database
        }
    }
    // not good
    do_login(LoginForm{username: form.username, password: form.password})
}

#[derive(Deserialize)]
struct LoginForm {
    username: String,
    password: String,
}


#[derive(Template)]
#[template(path = "login.html")] 
struct LoginTemplate<'a>{
    page: &'a str,
    login_failed: bool,
    global: Global<'a>,
} 

fn login_page() -> impl Reply {
    // dont let you access this page if logged in
    let global = Global::from_session(None); 
    render_template(&LoginTemplate{page: "login", login_failed: false, global:global})
}

fn do_login(form: LoginForm) -> impl Reply {
    if let Some(cookie) = Session::authenticate(&POOL.get().unwrap(), &form.username, &form.password) {
        http::Response::builder()
            .status(http::StatusCode::FOUND)
            .header(http::header::LOCATION, "/")
            .header(
                http::header::SET_COOKIE,
                format!("EXAUTH={}; SameSite=Strict; HttpOpnly", cookie),
        )
        .body(Body::empty()).unwrap()
    } else {
        let global = Global::from_session(None); 
        render_template(&LoginTemplate{page: "login", login_failed: true, global:global})
        // TODO -- better error handling
    }
}

fn do_logout(session: Option<Session>) -> impl Reply {
    use db::schema::sessions::dsl::*;
    if let Some(s) = session {
        diesel::delete(sessions.filter(id.eq(s.id))).execute(&POOL.get().unwrap()).unwrap();
    }
    warp::redirect::redirect(warp::http::Uri::from_static("/"))
}

fn render_timeline(session: Option<Session>) -> impl Reply {
    // no session -- anonymous
    let global = Global::from_session(session); 
    use db::schema::notes::dsl::*;
    // pulls a bunch of data i dont really need
    let results = notes
        .order(id.desc())
        .limit(250)
        .load::<Note>(&POOL.get().unwrap())
        .expect("Error loading posts");
    render_template(&TimelineTemplate{
        page: "timeline",
        global: global,
        notes: results,
    })

}

#[derive(Template)]
#[template(path = "server_info.html")]
struct ServerInfoTemplate<'a> {
    global: Global<'a>,
    page: &'a str,
}

#[derive(Template)]
#[template(path = "error.html")] 
struct ErrorTemplate<'a> {
    global: Global<'a>,
    page: &'a str,
    error_message: &'a str
}

#[derive(Template)]
#[template(path = "user.html")] 
struct UserTemplate<'a>{
    global: Global<'a>,
    page: &'a str,
    notes: Vec<Note>,
    user: User
} 

#[derive(Template)]
#[template(path = "note.html")] 
struct NoteTemplate<'a> {
    global: Global<'a>,
    page: &'a str,
    note: Note,
    // thread
}

fn server_info_page(session: Option<Session>) -> impl Reply {
    let global = Global::from_session(session); 
    render_template(&ServerInfoTemplate{global: global, page: "server"})
}

fn note_page(session: Option<Session>, note_id: i32) -> impl Reply {
    let global = Global::from_session(session); 
    use db::schema::notes::dsl::*;
    let conn = &POOL.get().unwrap();
    let note: Option<Note> = notes
        .filter(id.eq(note_id))
        .first::<Note>(conn)
        .ok();
    if let Some(n) = note {
        render_template(&NoteTemplate{global: global, note: n.clone(), page: &n.id.to_string()})
    }
    else {
        render_template(&ErrorTemplate{global: global, page: "error", error_message: "Note not found"})
    }
    // TODO -- fetch replies
}

fn user_page(session: Option<Session>, user_name: String) -> impl Reply {
    let global = Global::from_session(session); 
    use db::schema::users::dsl::*;
    let conn = &POOL.get().unwrap();
    let user: Option<User> = users
        .filter(username.eq(user_name))
        .first::<User>(conn)
        .ok();
    if let Some(u) = user {
        use db::schema::notes::dsl::*;
        let results = notes
            .filter(creator_id.eq(u.id))
            .order(id.desc())
            .load::<Note>(conn)
            .expect("Error loading posts");
        render_template(&UserTemplate{
            global: global,
            page: &u.username,
            user: u.clone(), // TODO stop cloning
            notes: results
        })
    }
    else {
        render_template(&ErrorTemplate{global: global, page: "error", error_message: "User not found"})
    }
}

async fn error_page(err: Rejection) -> Result<impl Reply, Infallible>{
    Ok(render_template(&ErrorTemplate{global: Global::from_session(None), page: "error", error_message: "You do not have access to this page."}))
}


// Url query
#[derive(Deserialize)]
struct Page {
    page_num: i32
}

pub async fn run_server() {
    env_logger::init();
    // cors filters etc
    
    // NOT TESTED YET
    let public = false; // std::env::var("PUBLIC").unwrap_or("false");
    let session_filter = move || session::create_session_filter(public).clone();

    use warp::{path, body::json, body::form, filters::query::query};

    let home = warp::path::end()
        .and(session_filter())
        .map(render_timeline);

    let user_page = session_filter()
        .and(path!("user" / String))
        .map(user_page);

    let note_page = session_filter()
        .and(path!("note" / i32))
        .map(note_page);

    let server_info_page = session_filter()
        .and(path("server_info"))
        .map(server_info_page);

    // auth functions
    let register_page = path("register")
        .and(query())
        .map(register_page);

    let do_register = path("do_register")
        .and(form())
        .and(query())
        .map(do_register);

    let login_page = path("login")
        .map(|| login_page());

    let do_login = path("do_login")
        .and(form())
        .map(do_login);

    let do_logout = path("logout")
        .and(session_filter())
        .map(do_logout);

    // CRUD actions
    let create_note = path("create_note")
        .and(session_filter())
        .and(form())
        .map(new_note);

    let delete_note = session_filter()
        .and(path!(i32 / "delete"))
        .map(delete_note);



    let static_files = warp::path("static")
        .and(warp::fs::dir("./static"));

    // activityPub stuff
    //
    // POST
    let post_user_inbox = path!("user" / String / "inbox.json" )
        .and(json())
        .map(ap::post_user_inbox);

    let post_user_outbox = path!("user" / String / "outbox.json" )
        .and(json())
        .map(ap::post_user_outbox);

    let get_user_outbox = path!("user" / String / "outbox.json" )
        .map(ap::get_user_outbox);

    // let get_user_inbox = path!("user" / String / "outbox.json" )
    //     .and(json())
    //     .map(ap::post_user_outbox);

    let user_followers = path!("user" / String / "followers.json" )
        .map(ap::user_followers);

    let user_following = path!("user" / String / "following.json" )
        .map(ap::user_following);

    // https://github.com/seanmonstar/warp/issues/42 -- how to set up diesel
    // TODO set content length limit 
    // TODO redirect via redirect in request
    // TODO secure against xss
        // used for api based authentication
    // let api_filter = session::create_session_filter(&POOL.get());
    let html_renders = home.or(login_page).or(register_page).or(user_page).or(note_page).or(server_info_page);
    let forms = login_page.or(do_register).or(do_login).or(create_note).or(delete_note).or(do_logout);
    // let api
    // catch all for any other paths

    let routes = warp::get().and(html_renders)
        .or(
            warp::post()
            .and(warp::body::content_length_limit(1024 * 32))
            .and(forms))
        .or(static_files)
        .with(warp::log("server"))
        .recover(error_page);

    match std::env::var("GOURAMI_ENV").unwrap().as_str() {
        "PROD" => warp::serve(routes)
            .tls()
            .cert_path(&std::env::var("CERT_PATH").unwrap())
            .key_path(&std::env::var("KEY_PATH").unwrap())
            .run(([0, 0, 0, 0], 443))
            .await ,
        _ => warp::serve(routes).run(([127,0,0,1], 3030)).await
    }
}
