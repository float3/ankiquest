(function(){
'use strict';
var aqText=AnkiQuestI18n.t;
var form=document.querySelector<HTMLFormElement>('#login-form')!,field=document.querySelector<HTMLInputElement>('#password')!,status=document.getElementById('login-status')!,button=form.querySelector('button')!;
var accounts=document.getElementById('accounts')!,community=document.querySelector<HTMLDetailsElement>('#community-login')!;
var signinForm=document.querySelector<HTMLFormElement>('#signin-form')!,signupForm=document.querySelector<HTMLFormElement>('#signup-form')!;
var signinTab=document.getElementById('tab-signin')!,signupTab=document.getElementById('tab-signup')!;
function destination(){
  var next=new URLSearchParams(location.search).get('next')||'/';
  if(!next.startsWith('/')||next.startsWith('//')||next.includes('\\')||/[\u0000-\u001f]/.test(next))next='/';
  try{var target=new URL(next,location.origin);if(target.origin!==location.origin||target.pathname==='/login'||target.pathname.startsWith('/auth/'))next='/';}catch(_){next='/';}
  if(!next.includes('#'))next+=location.hash;
  return next;
}
function input(form:HTMLFormElement,name:string){return form.querySelector<HTMLInputElement>(`[name="${name}"]`)!;}
function session(body:unknown,headers:Record<string,string>={}){
  return fetch('/auth/session',{method:'POST',credentials:'same-origin',headers:{'Content-Type':'application/json','X-Ankiquest-CSRF':'1',...headers},body:JSON.stringify(body)});
}
function showTab(signup:boolean){
  signinTab.setAttribute('aria-selected',String(!signup));signupTab.setAttribute('aria-selected',String(signup));
  signinForm.hidden=signup;signupForm.hidden=!signup;
  input(signup?signupForm:signinForm,'user').focus();
}
signinTab.addEventListener('click',function(){showTab(false);});
signupTab.addEventListener('click',function(){showTab(true);});
fetch('/auth/status',{credentials:'same-origin'}).then(function(response){return response.json();}).then(function(state:{registration?:boolean}){
  if(!state.registration)return;
  accounts.hidden=false;community.open=false;
  var note=document.querySelector<HTMLElement>('.login-note');if(note)note.hidden=true;
  var params=new URLSearchParams(location.search),nested=new URLSearchParams((params.get('next')||'').split('?')[1]||'');
  if(params.get('signup')==='1'||nested.get('signup')==='1')showTab(true);else input(signinForm,'user').focus();
}).catch(function(){});
signinForm.addEventListener('submit',async function(event){
  event.preventDefault();
  var submit=signinForm.querySelector('button')!,message=document.getElementById('signin-status')!;
  submit.disabled=true;await AnkiQuestI18n.ready;message.textContent=aqText("Signing in…");
  var password=input(signinForm,'password');
  try{
    var response=await session({user:input(signinForm,'user').value.trim().toLowerCase(),password:password.value});
    password.value='';
    if(response.ok){location.replace(destination());return;}
    message.textContent=response.status===429?aqText("Too many attempts. Wait one minute and try again."):response.status===401?aqText("Username or password not recognized."):aqText("Could not sign in. Please try again.");
  }catch(_){message.textContent=aqText("Could not reach the server. Check your connection and try again.");}
  submit.disabled=false;password.focus();
});
signupForm.addEventListener('submit',async function(event){
  event.preventDefault();
  var submit=signupForm.querySelector('button')!,message=document.getElementById('signup-status')!;
  submit.disabled=true;await AnkiQuestI18n.ready;message.textContent=aqText("Creating your account…");
  var user=input(signupForm,'user').value.trim().toLowerCase(),password=input(signupForm,'password');
  try{
    var response=await fetch('/api/accounts',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({user:user,password:password.value,display:input(signupForm,'display').value.trim(),device:'Website'})});
    password.value='';
    if(response.ok){
      var created:{user:string;token:string}=await response.json();
      document.getElementById('token-server')!.textContent=location.origin;
      document.getElementById('token-user')!.textContent=created.user;
      document.getElementById('token-value')!.textContent=created.token;
      signupForm.hidden=true;signinForm.hidden=true;document.querySelector<HTMLElement>('.login-tabs')!.hidden=true;community.hidden=true;
      var panel=document.getElementById('token-panel')!;panel.hidden=false;
      document.getElementById('token-continue')!.addEventListener('click',async function(){
        var signedIn=await session({},{Authorization:'Bearer '+created.token}).catch(function(){return null;});
        location.replace(signedIn&&signedIn.ok?destination():'/login');
      },{once:true});
      return;
    }
    message.textContent=response.status===409?aqText("That username is taken."):response.status===429?aqText("Too many new accounts from here. Try again later."):response.status===403?aqText("Sign-ups are closed on this server."):response.status===400?aqText("Check the username and password: usernames are 3 to 32 lowercase letters, digits, - or _, and passwords need at least 10 characters."):aqText("Could not create the account. Please try again.");
  }catch(_){message.textContent=aqText("Could not reach the server. Check your connection and try again.");}
  submit.disabled=false;
});
form.addEventListener('submit',async function(event){
  event.preventDefault();button.disabled=true;
  var password=field.value;field.value='';
  // The status messages are translated; the disabled button prevents a second submit while waiting.
  await AnkiQuestI18n.ready;
  status.textContent=aqText("Signing in…");
  try{
    var response=await session({password:password});
    password='';
    if(response.ok){location.replace(destination());return;}
    status.textContent=response.status===429?aqText("Too many attempts. Wait one minute and try again."):response.status===401?aqText("That password or token was not recognized. Please try again."):aqText("Could not sign in. Please try again.");
  }catch(_){password='';status.textContent=aqText("Could not reach the server. Check your connection and try again.");}
  button.disabled=false;field.focus();
});
})();
