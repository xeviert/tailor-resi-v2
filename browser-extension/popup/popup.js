// ResiTailor Extractor - Popup Script
console.log('ResiTailor popup loaded');

const extractBtn = document.getElementById('extract-btn');
const statusDiv = document.getElementById('status');

/**
 * Update the status message with appropriate styling
 * @param {string} message - The message to display
 * @param {string} type - 'success', 'error', or 'loading'
 */
function updateStatus(message, type = 'default') {
  statusDiv.textContent = message;
  statusDiv.className = '';
  
  if (type === 'success') {
    statusDiv.classList.add('status-success');
  } else if (type === 'error') {
    statusDiv.classList.add('status-error');
  } else if (type === 'loading') {
    statusDiv.classList.add('status-loading');
  }
}

/**
 * Set button to loading state
 * @param {boolean} isLoading - Whether button should be disabled
 */
function setLoadingState(isLoading) {
  extractBtn.disabled = isLoading;
  extractBtn.textContent = isLoading ? 'Extracting...' : 'Extract Job';
}

/**
 * Send extraction request to background script
 */
async function extractJob() {
  // Set loading state
  setLoadingState(true);
  updateStatus('Extracting job data...', 'loading');
  
  try {
    // Send message to background script
    const response = await chrome.runtime.sendMessage({ action: 'extractJob' });
    
    if (!response) {
      throw new Error('No response from background script');
    }
    
    if (response.success) {
      console.log('Extraction successful:', response);
      updateStatus('Job data sent successfully!', 'success');
    } else {
      // Handle different error types
      const errorMessage = response.error || 'Unknown error';
      
      if (errorMessage.includes('ECONNREFUSED') || errorMessage.includes('fetch')) {
        updateStatus('Cannot connect to backend. Make sure Tauri app is running on port 3000.', 'error');
      } else if (errorMessage.includes('timeout') || errorMessage.includes('Timeout')) {
        updateStatus('Backend took too long. Is it running?', 'error');
      } else if (errorMessage.includes('HTTP error')) {
        const statusMatch = errorMessage.match(/(\d{3})/);
        const statusCode = statusMatch ? statusMatch[1] : '';
        updateStatus(`Backend error: ${statusCode}`, 'error');
      } else {
        updateStatus(`Error: ${errorMessage}`, 'error');
      }
    }
  } catch (error) {
    console.error('Extraction error:', error);
    
    if (error.message.includes('No active tab')) {
      updateStatus('No active tab found. Please open a job posting page.', 'error');
    } else if (error.message.includes('Could not establish connection')) {
      updateStatus('Extension context invalidated. Please reload the extension.', 'error');
    } else {
      updateStatus('Failed to extract job data. Please try again.', 'error');
    }
  } finally {
    // Re-enable button for retry
    setLoadingState(false);
  }
}

// Set up event listener
if (extractBtn && statusDiv) {
  extractBtn.addEventListener('click', () => {
    extractJob();
  });
  
  // Initialize status
  updateStatus('Click to extract job data');
} else {
  console.error('Could not find extract button or status element');
}
