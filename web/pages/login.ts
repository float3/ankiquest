(function(){
'use strict';
var aqText=AnkiQuestI18n.t;
var form=document.querySelector<HTMLFormElement>('#login-form')!,field=document.querySelector<HTMLInputElement>('#password')!,status=document.getElementById('login-status')!,button=form.querySelector('button')!;
function destination(){
  var next=new URLSearchParams(location.search).get('next')||'/';
  if(!next.startsWith('/')||next.startsWith('//')||next.includes('\\')||/[\u0000-\u001f]/.test(next))next='/';
  try{var target=new URL(next,location.origin);if(target.origin!==location.origin||target.pathname==='/login'||target.pathname.startsWith('/auth/'))next='/';}catch(_){next='/';}
  if(!next.includes('#'))next+=location.hash;
  return next;
}
form.addEventListener('submit',async function(event){
  event.preventDefault();button.disabled=true;
  var password=field.value;field.value='';
  // The status messages are translated; the disabled button prevents a second submit while waiting.
  await AnkiQuestI18n.ready;
  status.textContent=aqText("Signing in…");
  try{
    var response=await fetch('/auth/session',{method:'POST',credentials:'same-origin',headers:{'Content-Type':'application/json','X-Ankiquest-CSRF':'1'},body:JSON.stringify({password:password})});
    password='';
    if(response.ok){location.replace(destination());return;}
    status.textContent=response.status===429?aqText("Too many attempts. Wait one minute and try again."):response.status===401?aqText("That password or token was not recognized. Please try again."):aqText("Could not sign in. Please try again.");
  }catch(_){password='';status.textContent=aqText("Could not reach the server. Check your connection and try again.");}
  button.disabled=false;field.focus();
});
})();
